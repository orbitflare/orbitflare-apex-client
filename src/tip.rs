//! Building the tip instruction apex-sender requires.

use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// Floor of the standard tier, in lamports (0.001 SOL). Your key's tier may
/// set a different floor; the admission response says so.
pub const MIN_TIP_LAMPORTS: u64 = 1_000_000;

/// A SystemProgram transfer from `payer` to one of the published tip
/// accounts. Include it as a top-level instruction of the transaction; the
/// tip account must be a static account key (not in a lookup table).
pub fn tip_instruction(payer: &Pubkey, tip_account: &Pubkey, lamports: u64) -> Instruction {
    solana_system_interface::instruction::transfer(payer, tip_account, lamports)
}

/// Pick one of the published tip accounts at random.
pub fn pick_tip_account(tip_accounts: &[Pubkey]) -> Option<Pubkey> {
    use rand::seq::SliceRandom;
    tip_accounts.choose(&mut rand::thread_rng()).copied()
}
