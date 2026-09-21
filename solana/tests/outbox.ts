// Localnet end-to-end: the real SBF build of base_caller running on
// solana-test-validator, driving prepare -> dispatch_via_wormhole -> finalize
// against a mock transport program. Run with `anchor test`.
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram, SYSVAR_CLOCK_PUBKEY, SYSVAR_RENT_PUBKEY } from "@solana/web3.js";
import { expect } from "chai";
import { BaseCaller } from "../target/types/base_caller";
import { MockTransport } from "../target/types/mock_transport";

const TRANSPORT_WORMHOLE = 1;
const MASK_WORMHOLE = 1 << 0;
const CHAIN_BASE = 3;
const HEADER_SIZE = 103;

describe("base_caller on localnet", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.BaseCaller as Program<BaseCaller>;
  const mock = anchor.workspace.MockTransport as Program<MockTransport>;
  const payer = (provider.wallet as anchor.Wallet).payer;

  const [config] = PublicKey.findProgramAddressSync([Buffer.from("config")], program.programId);
  const [transport] = PublicKey.findProgramAddressSync(
    [Buffer.from("transport"), Buffer.from([TRANSPORT_WORMHOLE])], program.programId);
  const [emitter] = PublicKey.findProgramAddressSync([Buffer.from("emitter")], program.programId);

  // Bridge-owned PDAs, derived from the configured bridge program id (here the
  // mock). The program derives the same addresses and rejects anything else.
  const [bridgePda] = PublicKey.findProgramAddressSync([Buffer.from("Bridge")], mock.programId);
  const [feeCollectorPda] = PublicKey.findProgramAddressSync([Buffer.from("fee_collector")], mock.programId);
  const [sequencePda] = PublicKey.findProgramAddressSync(
    [Buffer.from("Sequence"), emitter.toBuffer()], mock.programId);

  const authority = Keypair.generate();
  const senderState = PublicKey.findProgramAddressSync(
    [Buffer.from("sender"), authority.publicKey.toBuffer()], program.programId)[0];
  const messagePda = (nonce: number) => {
    const n = Buffer.alloc(8); n.writeBigUInt64LE(BigInt(nonce));
    return PublicKey.findProgramAddressSync(
      [Buffer.from("msg"), authority.publicKey.toBuffer(), n], program.programId)[0];
  };

  const params = {
    dstChainId: CHAIN_BASE,
    target: Array.from(Buffer.alloc(20, 0xab)),
    value: new anchor.BN(0),
    gasLimit: new anchor.BN(200_000),
    expiry: new anchor.BN(0),
    mode: 0,
    calldata: Buffer.from([0xde, 0xad, 0xbe, 0xef]),
  };

  it("initializes and registers the (mock) wormhole transport", async () => {
    await program.methods.initialize(payer.publicKey)
      .accountsPartial({ config, payer: payer.publicKey, systemProgram: SystemProgram.programId })
      .rpc();
    await program.methods.setTransport(TRANSPORT_WORMHOLE, true, mock.programId, Array.from(Buffer.alloc(32, 0xaa)), 30)
      .accountsPartial({ config, transport, admin: payer.publicKey, systemProgram: SystemProgram.programId })
      .rpc();
    const t = await program.account.transportConfig.fetch(transport);
    expect(t.enabled).to.eq(true);
    expect(t.programId.toBase58()).to.eq(mock.programId.toBase58());
  });

  it("prepare: nonce 1, envelope stored with the right header", async () => {
    await program.methods.prepare(params, MASK_WORMHOLE)
      .accountsPartial({
        config, senderState, message: messagePda(1),
        authority: authority.publicKey, payer: payer.publicKey, systemProgram: SystemProgram.programId,
      })
      .signers([authority])
      .rpc();
    const ss = await program.account.senderState.fetch(senderState);
    expect(ss.nonce.toNumber()).to.eq(1);
    const m = await program.account.preparedMessage.fetch(messagePda(1));
    expect(m.expected).to.eq(MASK_WORMHOLE);
    expect(m.dispatched).to.eq(0);
    const env = Buffer.from(m.envelope);
    expect(env.length).to.eq(HEADER_SIZE + 4);
    expect(env[0]).to.eq(1);                                   // version
    expect(env.readUInt16BE(2)).to.eq(1);                      // srcChainId = Solana
    expect(env.readUInt16BE(4)).to.eq(CHAIN_BASE);             // dstChainId
    expect(env.subarray(6, 38).equals(authority.publicKey.toBuffer())).to.eq(true);
    expect(Number(env.readBigUInt64BE(38))).to.eq(1);          // nonce
    expect(env.subarray(HEADER_SIZE).equals(Buffer.from([0xde, 0xad, 0xbe, 0xef]))).to.eq(true);
  });

  it("dispatch_via_wormhole: CPI with PDA-signed emitter succeeds, bit set, repeat refused", async () => {
    const whMessage = Keypair.generate();
    const dispatch = () => program.methods.dispatchViaWormhole(new anchor.BN(1), 0)
      .accountsPartial({
        config, transport, message: messagePda(1), authority: authority.publicKey, payer: payer.publicKey,
        wormholeBridge: bridgePda,
        wormholeMessage: whMessage.publicKey,
        wormholeEmitter: emitter,
        wormholeSequence: sequencePda,
        wormholeFeeCollector: feeCollectorPda,
        wormholeProgram: mock.programId,
        clock: SYSVAR_CLOCK_PUBKEY, rent: SYSVAR_RENT_PUBKEY, systemProgram: SystemProgram.programId,
      })
      .signers([whMessage])
      .rpc();
    await dispatch();
    const m = await program.account.preparedMessage.fetch(messagePda(1));
    expect(m.dispatched).to.eq(MASK_WORMHOLE);

    let threw = false;
    try { await dispatch(); } catch (e: any) {
      threw = true;
      expect(String(e)).to.contain("AlreadyDispatched");
    }
    expect(threw).to.eq(true);
  });

  it("finalize: closes the message and refunds rent; next prepare is nonce 2", async () => {
    await program.methods.finalize(new anchor.BN(1))
      .accountsPartial({ message: messagePda(1), authority: authority.publicKey, payer: payer.publicKey })
      .rpc();
    const closed = await provider.connection.getAccountInfo(messagePda(1));
    expect(closed).to.eq(null);

    await program.methods.prepare(params, MASK_WORMHOLE)
      .accountsPartial({
        config, senderState, message: messagePda(2),
        authority: authority.publicKey, payer: payer.publicKey, systemProgram: SystemProgram.programId,
      })
      .signers([authority])
      .rpc();
    const ss = await program.account.senderState.fetch(senderState);
    expect(ss.nonce.toNumber()).to.eq(2);
  });
});
