import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Vault } from "../target/types/vault";

describe("vault", () => {
  // Configure the client to use the local cluster.
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  // Pull in our program as typed
  const program = anchor.workspace.Vault as Program<Vault>;

  it("initializes", async () => {
    await program.methods
      .initialize()
      .rpc();
    console.log("Vault initialized at", program.programId.toBase58());
  });
});
