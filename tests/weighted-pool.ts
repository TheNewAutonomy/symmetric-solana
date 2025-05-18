import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { WeightedPool } from "../target/types/weighted_pool";

describe("weighted-pool", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.WeightedPool as Program<WeightedPool>;

  it("initializes", async () => {
    await program.methods
      .initialize()
      .rpc();
    console.log("WeightedPool initialized at", program.programId.toBase58());
  });
});
