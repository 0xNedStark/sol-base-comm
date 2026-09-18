// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Auth} from "./Auth.sol";
import {EnvelopeLib} from "./libraries/EnvelopeLib.sol";
import {ISolanaGateway} from "./interfaces/ISolanaGateway.sol";
import {ISolanaAccount} from "./interfaces/ISolanaAccount.sol";
import {SolanaAccount} from "./SolanaAccount.sol";

/// @title SolanaGateway
/// @notice Single destination-chain entry point for calls that originated on
///         Solana. Phase one: DIRECT mode, no native value, one or more
///         adapters with per-target quorum.
///
/// Responsibilities, and deliberately nothing else:
///   - accept envelopes only from registered adapters;
///   - classify every delivery into exactly one of: duplicate, terminal
///     rejection, parked failure, transient revert, or execution;
///   - deduplicate by messageId and enforce the per-target adapter quorum;
///   - execute, exposing the originating Solana pubkey to the target.
///
/// It does NOT decide whether a given Solana key may call a given function.
/// That belongs in the target contract (see SolanaCallable). Strict mode is
/// optional hardening that stacks on top of the target's own check.
///
/// The one rule that shapes the control flow: revert only when a later
/// redelivery could succeed with no change on our side (paused, under-gassed).
/// Anything else is consumed and recorded, because some transports respond to
/// a reverting receive by queueing it for redelivery, turning one bad message
/// into an unbounded retry loop.
contract SolanaGateway is ISolanaGateway, Auth {
    using EnvelopeLib for bytes;
    using EnvelopeLib for EnvelopeLib.Envelope;

    bytes32 private constant _T_SENDER = keccak256("solana-base-comm.xdomain.sender");
    bytes32 private constant _T_NONCE = keccak256("solana-base-comm.xdomain.nonce");

    uint256 private constant GAS_RESERVE = 40_000;
    uint256 private constant ACCOUNT_FRAME_OVERHEAD = 40_000;

    /// @notice This gateway's own internal chain id. Every envelope names its
    ///         destination; one meant for another chain is rejected here.
    uint16 public immutable chainId;

    address public immutable accountImplementation;

    /// @notice Envelope versions this gateway will execute. During a format
    ///         upgrade, accept the new version before the source emits it and
    ///         retire the old one after its longest in-flight window.
    mapping(uint8 => bool) public acceptedVersion;

    /// @notice ACCOUNT mode ships in a later phase. Off by default; a message
    ///         requesting it is parked, not rejected, so enabling the mode
    ///         later lets it run.
    bool public accountModeEnabled;

    mapping(address => bool) public isAdapter;

    /// @notice Every SolanaAccount this gateway has deployed. A DIRECT-mode
    ///         call must never target one: the gateway is msg.sender in that
    ///         mode, and the account's `execute` trusts exactly that caller.
    mapping(address => bool) public isAccount;

    mapping(bytes32 => Status) internal _status;
    mapping(bytes32 => uint256) public confirmations;
    mapping(bytes32 => mapping(address => bool)) public confirmedBy;

    /// @notice Adapters that must independently deliver a message before it
    ///         executes. 0 is treated as 1. The only control here that
    ///         addresses transport compromise.
    mapping(address => uint8) public requiredConfirmations;

    mapping(address => bool) public strictMode;
    mapping(bytes32 => bool) public allowedCall;

    event AdapterSet(address indexed adapter, bool enabled);
    event AcceptedVersionSet(uint8 version, bool accepted);
    event AccountModeSet(bool enabled);
    event RequiredConfirmationsSet(address indexed target, uint8 required);
    event StrictModeSet(address indexed target, bool enabled);
    event AllowedCallSet(address indexed target, bytes32 indexed sender, bytes4 selector, bool allowed);

    error NotAdapter();
    error InsufficientGas(uint256 available, uint256 required);
    error NotFailed();
    error GasLimitTooLow();
    error Create2Mismatch();

    constructor(address owner_, uint16 chainId_) Auth(owner_) {
        chainId = chainId_;
        accountImplementation = address(new SolanaAccount(address(this)));
        acceptedVersion[EnvelopeLib.VERSION] = true;
        emit AcceptedVersionSet(EnvelopeLib.VERSION, true);
    }

    // ---------------------------------------------------------------- delivery

    /// @inheritdoc ISolanaGateway
    function deliver(bytes calldata envelope) external nonReentrant whenNotPaused {
        if (!isAdapter[msg.sender]) revert NotAdapter();

        bytes32 id = keccak256(envelope);

        Status st = _status[id];
        if (st == Status.Executed || st == Status.Failed || st == Status.Rejected || confirmedBy[id][msg.sender]) {
            emit MessageDuplicate(id, msg.sender);
            return;
        }

        (bool decoded, EnvelopeLib.Envelope memory e) = EnvelopeLib.tryDecode(envelope);
        if (!decoded) {
            _reject(id, Reason.Malformed);
            return;
        }

        Reason r = _terminalCheck(e);
        if (r != Reason.None) {
            _reject(id, r);
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

        r = _parkableCheck(e);
        if (r != Reason.None) {
            _park(id, e.target, r, "");
            return;
        }

        _run(id, e, e.gasLimit);
    }

    /// @inheritdoc ISolanaGateway
    function retry(bytes calldata envelope, uint64 gasLimitOverride) external nonReentrant whenNotPaused {
        bytes32 id = keccak256(envelope);
        if (_status[id] != Status.Failed) revert NotFailed();

        // A Failed message decoded once; it decodes again. Re-run the terminal
        // checks anyway -- it may have expired while parked.
        (bool decoded, EnvelopeLib.Envelope memory e) = EnvelopeLib.tryDecode(envelope);
        if (!decoded) {
            _reject(id, Reason.Malformed);
            return;
        }
        Reason r = _terminalCheck(e);
        if (r != Reason.None) {
            _reject(id, r);
            return;
        }
        r = _parkableCheck(e);
        if (r != Reason.None) {
            _park(id, e.target, r, "");
            return;
        }

        uint64 gasLimit = e.gasLimit;
        if (gasLimitOverride != 0) {
            if (gasLimitOverride < e.gasLimit) revert GasLimitTooLow();
            gasLimit = gasLimitOverride;
        }

        _run(id, e, gasLimit);
    }

    /// @dev Conditions no retry can cure.
    function _terminalCheck(EnvelopeLib.Envelope memory e) internal view returns (Reason) {
        if (!acceptedVersion[e.version]) return Reason.BadVersion;
        if (e.msgType != EnvelopeLib.MSG_TYPE_CALL) return Reason.BadMessageType;
        if (e.srcChainId != EnvelopeLib.CHAIN_ID_SOLANA) return Reason.BadSourceChain;
        if (e.dstChainId != chainId) return Reason.BadDestinationChain;
        if (e.mode > EnvelopeLib.MODE_ACCOUNT) return Reason.BadMode;
        if (e.expiry != 0 && block.timestamp > e.expiry) return Reason.Expired;
        if (e.value != 0) return Reason.ValueNotSupported;
        if (_isForbiddenTarget(e.target)) return Reason.ForbiddenTarget;
        return Reason.None;
    }

    /// @dev Conditions an operator can change, after which anyone retries.
    function _parkableCheck(EnvelopeLib.Envelope memory e) internal view returns (Reason) {
        if (e.mode == EnvelopeLib.MODE_ACCOUNT && !accountModeEnabled) return Reason.AccountModeDisabled;
        if (strictMode[e.target] && !allowedCall[_callKey(e.target, e.sender, e.selector())]) {
            return Reason.NotAllowed;
        }
        return Reason.None;
    }

    /// @dev The set of addresses a DIRECT call must never reach: anything the
    ///      gateway itself is privileged over. Kept empty of everything else
    ///      by policy -- the gateway holds no approvals and no roles.
    function _isForbiddenTarget(address t) internal view returns (bool) {
        return t == address(0) || t == address(this) || t == accountImplementation || isAdapter[t] || isAccount[t];
    }

    function _reject(bytes32 id, Reason r) internal {
        _status[id] = Status.Rejected;
        emit MessageRejected(id, r);
    }

    function _park(bytes32 id, address target, Reason r, bytes memory ret) internal {
        _status[id] = Status.Failed;
        emit CallFailed(id, target, r, ret);
    }

    function _run(bytes32 id, EnvelopeLib.Envelope memory e, uint64 gasLimit) internal {
        // Effects before interaction: mark consumed, then walk it back only on
        // a failure we have already observed.
        _status[id] = Status.Executed;

        // Resolve -- and on first use, deploy -- the account BEFORE measuring
        // gas, so the deployment cannot eat into the certified budget.
        address account;
        if (e.mode == EnvelopeLib.MODE_ACCOUNT) account = _ensureAccount(e.sender);

        // The floor sits immediately before the call. This is the one
        // condition in the execution path that reverts: an under-gassed
        // delivery is the relayer's fault and a redelivery with more gas will
        // succeed, so consuming the message here would be wrong.
        uint256 needed = _gasFloor(gasLimit, e.mode);
        if (gasleft() < needed) revert InsufficientGas(gasleft(), needed);

        _setTransient(e.sender, e.nonce);

        bool ok;
        bytes memory ret;
        if (e.mode == EnvelopeLib.MODE_DIRECT) {
            (ok, ret) = e.target.call{gas: gasLimit}(e.callData);
        } else {
            (ok, ret) = ISolanaAccount(account).execute(e.target, 0, e.callData, gasLimit);
        }

        _setTransient(bytes32(0), 0);

        if (ok) {
            emit CallExecuted(id, e.sender, e.target, e.nonce);
        } else {
            _park(id, e.target, Reason.InnerCallReverted, ret);
        }
    }

    /// @dev Gas the gateway must hold so the target receives at least
    ///      `gasLimit`. Each CALL forwards at most 63/64 of what remains, so
    ///      every frame between here and the target needs its own 64/63
    ///      inflation plus its own overhead.
    function _gasFloor(uint64 gasLimit, uint8 mode) internal pure returns (uint256 needed) {
        needed = (uint256(gasLimit) * 64) / 63;
        if (mode == EnvelopeLib.MODE_ACCOUNT) {
            needed = ((needed + ACCOUNT_FRAME_OVERHEAD) * 64) / 63;
        }
        needed += GAS_RESERVE;
    }

    // ----------------------------------------------------------- xdomain view

    function xDomainMessageSender() public view returns (bytes32 s) {
        bytes32 slot = _T_SENDER;
        assembly {
            s := tload(slot)
        }
    }

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
        isAccount[deployed] = true;
        emit AccountDeployed(sender, deployed);
    }

    // ------------------------------------------------------------------ views

    function statusOf(bytes32 id) external view returns (Status) {
        return _status[id];
    }

    // ------------------------------------------------------------------ admin

    function setAdapter(address adapter, bool enabled) external onlyOwner {
        isAdapter[adapter] = enabled;
        emit AdapterSet(adapter, enabled);
    }

    function setAcceptedVersion(uint8 version, bool accepted) external onlyOwner {
        acceptedVersion[version] = accepted;
        emit AcceptedVersionSet(version, accepted);
    }

    function setAccountModeEnabled(bool enabled) external onlyOwner {
        accountModeEnabled = enabled;
        emit AccountModeSet(enabled);
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
