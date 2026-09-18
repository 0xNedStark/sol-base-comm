// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Auth} from "./Auth.sol";
import {EnvelopeLib} from "./libraries/EnvelopeLib.sol";
import {ISolanaGateway} from "./interfaces/ISolanaGateway.sol";
import {ISolanaAccount} from "./interfaces/ISolanaAccount.sol";
import {SolanaAccount} from "./SolanaAccount.sol";

/// @title SolanaGateway
/// @notice Single Base-side entry point for calls that originated on Solana.
///
/// Responsibilities, and deliberately nothing else:
///   - accept envelopes only from registered adapters;
///   - enforce version, source chain and expiry;
///   - deduplicate by messageId and enforce the per-target adapter quorum;
///   - execute, exposing the originating Solana pubkey to the target;
///   - park a reverted call for permissionless retry rather than losing it.
///
/// It does NOT decide whether a given Solana key may call a given function.
/// That belongs in the target contract (see SolanaCallable), for the reasons in
/// D1 of docs/01-architecture.md. Strict mode below is optional hardening that
/// stacks on top of the target's own check, never a replacement for it.
contract SolanaGateway is ISolanaGateway, Auth {
    using EnvelopeLib for bytes;
    using EnvelopeLib for EnvelopeLib.Envelope;

    /// @dev Transient slots (EIP-1153). Base has supported TSTORE/TLOAD since
    ///      the Cancun upgrade. Cleared after every execution.
    bytes32 private constant _T_SENDER = keccak256("solana-base-comm.xdomain.sender");
    bytes32 private constant _T_NONCE = keccak256("solana-base-comm.xdomain.nonce");

    /// @dev Headroom left for bookkeeping after the inner call returns.
    uint256 private constant GAS_RESERVE = 40_000;

    /// @dev Cost of the extra frame in ACCOUNT mode: the gateway -> account
    ///      call itself, its calldata copy, the onlyGateway check, and
    ///      returning the target's return data. Generous on purpose.
    uint256 private constant ACCOUNT_FRAME_OVERHEAD = 40_000;

    /// @notice This gateway's own internal chain id. Every envelope names its
    ///         destination; one meant for another chain is rejected here, so a
    ///         second deployment elsewhere cannot be fed this chain's messages.
    uint16 public immutable chainId;

    /// @notice Implementation cloned for each ACCOUNT-mode sender.
    address public accountImplementation;

    mapping(address => bool) public isAdapter;

    mapping(bytes32 => Status) internal _status;
    mapping(bytes32 => uint256) public confirmations;
    mapping(bytes32 => mapping(address => bool)) public confirmedBy;

    /// @notice Adapters that must independently deliver a message before it
    ///         executes. 0 is treated as 1. Set to 2 for high-value targets to
    ///         defend against compromise of a single transport -- the only
    ///         control here that does; see docs/03-transport-comparison.md.
    mapping(address => uint8) public requiredConfirmations;

    /// @notice Opt-in per-target hardening. When on, the gateway additionally
    ///         requires the (target, sender, selector) triple to be allowlisted.
    mapping(address => bool) public strictMode;
    mapping(bytes32 => bool) public allowedCall;

    /// @notice ETH held on behalf of a Solana sender, spent by `value`.
    mapping(bytes32 => uint256) internal _balance;

    event AdapterSet(address indexed adapter, bool enabled);
    event RequiredConfirmationsSet(address indexed target, uint8 required);
    event StrictModeSet(address indexed target, bool enabled);
    event AllowedCallSet(address indexed target, bytes32 indexed sender, bytes4 selector, bool allowed);
    event Withdrawn(bytes32 indexed sender, address indexed to, uint256 amount);

    error NotAdapter();
    error BadVersion(uint8 version);
    error BadMessageType(uint8 msgType);
    error BadSourceChain(uint16 srcChainId);
    error BadDestinationChain(uint16 dstChainId, uint16 expected);
    error MessageExpired(uint64 expiry);
    error BadMode(uint8 mode);
    error NotAllowed();
    error InsufficientGas(uint256 available, uint256 required);
    error NotFailed();
    error GasLimitTooLow();
    error NotSelf();
    error InsufficientBalance();
    error TransferFailed();
    error Create2Mismatch();

    constructor(address owner_, uint16 chainId_) Auth(owner_) {
        chainId = chainId_;
        accountImplementation = address(new SolanaAccount(address(this)));
    }

    // ---------------------------------------------------------------- delivery

    /// @inheritdoc ISolanaGateway
    function deliver(bytes calldata envelope) external nonReentrant whenNotPaused {
        if (!isAdapter[msg.sender]) revert NotAdapter();

        EnvelopeLib.Envelope memory e = EnvelopeLib.decode(envelope);
        _validate(e);

        bytes32 id = keccak256(envelope);

        // Already settled, or this adapter has spoken before. Return rather
        // than revert: several transports queue a reverting receive for
        // redelivery, which would turn one duplicate into a retry loop.
        if (_status[id] == Status.Executed || _status[id] == Status.Failed || confirmedBy[id][msg.sender]) {
            emit MessageDuplicate(id, msg.sender);
            return;
        }

        confirmedBy[id][msg.sender] = true;
        uint256 n = ++confirmations[id];
        emit MessageDelivered(id, msg.sender, n);

        uint8 required = requiredConfirmations[e.target];
        if (required == 0) required = 1;
        if (n < required) {
            _status[id] = Status.Pending;
            return;
        }

        if (strictMode[e.target] && !allowedCall[_callKey(e.target, e.sender, e.selector())]) {
            revert NotAllowed();
        }

        _run(id, e, e.gasLimit);
    }

    /// @inheritdoc ISolanaGateway
    function retry(bytes calldata envelope, uint64 gasLimitOverride) external nonReentrant whenNotPaused {
        bytes32 id = keccak256(envelope);
        if (_status[id] != Status.Failed) revert NotFailed();

        EnvelopeLib.Envelope memory e = EnvelopeLib.decode(envelope);
        _validate(e);

        uint64 gasLimit = e.gasLimit;
        if (gasLimitOverride != 0) {
            if (gasLimitOverride < e.gasLimit) revert GasLimitTooLow();
            gasLimit = gasLimitOverride;
        }

        _run(id, e, gasLimit);
    }

    function _validate(EnvelopeLib.Envelope memory e) internal view {
        if (e.version != EnvelopeLib.VERSION) revert BadVersion(e.version);
        if (e.msgType != EnvelopeLib.MSG_TYPE_CALL) revert BadMessageType(e.msgType);
        if (e.srcChainId != EnvelopeLib.CHAIN_ID_SOLANA) revert BadSourceChain(e.srcChainId);
        if (e.dstChainId != chainId) revert BadDestinationChain(e.dstChainId, chainId);
        if (e.expiry != 0 && block.timestamp > e.expiry) revert MessageExpired(e.expiry);
        if (e.mode > EnvelopeLib.MODE_ACCOUNT) revert BadMode(e.mode);
    }

    function _run(bytes32 id, EnvelopeLib.Envelope memory e, uint64 gasLimit) internal {
        uint256 value = e.value;
        if (value != 0) {
            if (_balance[e.sender] < value) {
                // Not a revert: someone can top the sender up and retry.
                _status[id] = Status.Failed;
                emit CallFailed(id, e.target, abi.encodeWithSelector(InsufficientBalance.selector));
                return;
            }
            _balance[e.sender] -= value;
        }

        // Effects before interaction: mark consumed, then walk it back only on
        // a failure we have already observed.
        _status[id] = Status.Executed;

        // Resolve -- and on a sender's first message, deploy -- the account
        // BEFORE measuring gas. A deployment that ran after the floor check
        // would eat into the budget the check just certified, and first-use
        // messages would reach the target with less than `gasLimit`.
        address account;
        if (e.mode == EnvelopeLib.MODE_ACCOUNT) account = _ensureAccount(e.sender);

        // The floor sits immediately before the call, after every piece of
        // gateway bookkeeping, so nothing between here and the CALL can erode
        // it. A relayer must not be able to submit a delivery with just barely
        // too little gas, have the inner call run out inside its 63/64
        // allowance, and burn the message as failed for the price of one tx.
        uint256 needed = _gasFloor(gasLimit, e.mode);
        if (gasleft() < needed) revert InsufficientGas(gasleft(), needed);

        _setTransient(e.sender, e.nonce);

        bool ok;
        bytes memory ret;
        if (e.mode == EnvelopeLib.MODE_DIRECT) {
            (ok, ret) = e.target.call{value: value, gas: gasLimit}(e.callData);
        } else {
            (ok, ret) = ISolanaAccount(account).execute{value: value}(e.target, value, e.callData, gasLimit);
        }

        _setTransient(bytes32(0), 0);

        if (ok) {
            emit CallExecuted(id, e.sender, e.target, e.nonce);
        } else {
            _status[id] = Status.Failed;
            if (value != 0) _balance[e.sender] += value; // refund for the retry
            emit CallFailed(id, e.target, ret);
        }
    }

    /// @dev Gas the gateway must hold so the target receives at least
    ///      `gasLimit`. Each CALL forwards at most 63/64 of what remains, so
    ///      every frame between here and the target needs its own 64/63
    ///      inflation plus its own overhead. DIRECT has one frame; ACCOUNT has
    ///      two (gateway -> account -> target).
    function _gasFloor(uint64 gasLimit, uint8 mode) internal pure returns (uint256 needed) {
        needed = (uint256(gasLimit) * 64) / 63;
        if (mode == EnvelopeLib.MODE_ACCOUNT) {
            needed = ((needed + ACCOUNT_FRAME_OVERHEAD) * 64) / 63;
        }
        needed += GAS_RESERVE;
    }

    // ----------------------------------------------------------- xdomain view

    /// @inheritdoc ISolanaGateway
    function xDomainMessageSender() public view returns (bytes32 s) {
        bytes32 slot = _T_SENDER;
        assembly {
            s := tload(slot)
        }
    }

    /// @inheritdoc ISolanaGateway
    function xDomainMessageNonce() public view returns (uint64 n) {
        bytes32 slot = _T_NONCE;
        assembly {
            n := tload(slot)
        }
    }

    function _setTransient(bytes32 sender, uint64 nonce) internal {
        bytes32 s1 = _T_SENDER;
        bytes32 s2 = _T_NONCE;
        assembly {
            tstore(s1, sender)
            tstore(s2, nonce)
        }
    }

    // --------------------------------------------------------------- accounts

    /// @inheritdoc ISolanaGateway
    function accountFor(bytes32 sender) public view returns (address predicted) {
        bytes32 salt = keccak256(abi.encodePacked(EnvelopeLib.CHAIN_ID_SOLANA, sender));
        address impl = accountImplementation;
        address deployer = address(this);
        assembly {
            let ptr := mload(0x40)
            mstore(ptr, 0x3d602d80600a3d3981f3363d3d373d3d3d363d73000000000000000000000000)
            mstore(add(ptr, 0x14), shl(0x60, impl))
            mstore(add(ptr, 0x28), 0x5af43d82803e903d91602b57fd5bf3ff00000000000000000000000000000000)
            mstore(add(ptr, 0x38), shl(0x60, deployer))
            mstore(add(ptr, 0x4c), salt)
            mstore(add(ptr, 0x6c), keccak256(ptr, 0x37))
            predicted := and(keccak256(add(ptr, 0x37), 0x55), 0xffffffffffffffffffffffffffffffffffffffff)
        }
    }

    function _ensureAccount(bytes32 sender) internal returns (address account) {
        account = accountFor(sender);
        if (account.code.length != 0) return account;

        bytes32 salt = keccak256(abi.encodePacked(EnvelopeLib.CHAIN_ID_SOLANA, sender));
        address impl = accountImplementation;
        address deployed;
        assembly {
            let ptr := mload(0x40)
            mstore(ptr, 0x3d602d80600a3d3981f3363d3d373d3d3d363d73000000000000000000000000)
            mstore(add(ptr, 0x14), shl(0x60, impl))
            mstore(add(ptr, 0x28), 0x5af43d82803e903d91602b57fd5bf30000000000000000000000000000000000)
            deployed := create2(0, ptr, 0x37, salt)
        }
        if (deployed != account) revert Create2Mismatch();
        ISolanaAccount(deployed).initialize(sender);
        emit AccountDeployed(sender, deployed);
    }

    // ------------------------------------------------------------------ funds

    /// @inheritdoc ISolanaGateway
    function deposit(bytes32 sender) external payable {
        _balance[sender] += msg.value;
        emit Deposited(sender, msg.sender, msg.value);
    }

    /// @notice Withdraw a sender's balance. Reachable only as the target of a
    ///         DIRECT-mode message from that same sender, so the Solana side
    ///         stays in control of its own funds and nothing gets stranded.
    function withdraw(address to, uint256 amount) external {
        if (msg.sender != address(this)) revert NotSelf();
        bytes32 sender = xDomainMessageSender();
        if (_balance[sender] < amount) revert InsufficientBalance();
        _balance[sender] -= amount;
        (bool ok,) = to.call{value: amount}("");
        if (!ok) revert TransferFailed();
        emit Withdrawn(sender, to, amount);
    }

    function statusOf(bytes32 id) external view returns (Status) {
        return _status[id];
    }

    function balanceOf(bytes32 sender) external view returns (uint256) {
        return _balance[sender];
    }

    // ------------------------------------------------------------------ admin

    function setAdapter(address adapter, bool enabled) external onlyOwner {
        isAdapter[adapter] = enabled;
        emit AdapterSet(adapter, enabled);
    }

    function setRequiredConfirmations(address target, uint8 required) external onlyOwner {
        requiredConfirmations[target] = required;
        emit RequiredConfirmationsSet(target, required);
    }

    function setStrictMode(address target, bool enabled) external onlyOwner {
        strictMode[target] = enabled;
        emit StrictModeSet(target, enabled);
    }

    function setAllowedCall(address target, bytes32 sender, bytes4 sel, bool allowed) external onlyOwner {
        allowedCall[_callKey(target, sender, sel)] = allowed;
        emit AllowedCallSet(target, sender, sel, allowed);
    }

    function _callKey(address target, bytes32 sender, bytes4 sel) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(target, sender, sel));
    }
}
