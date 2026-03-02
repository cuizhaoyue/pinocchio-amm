//! Pinocchio AMM 程序入口。
//!
//! 这个文件只做两件事：
//! 1) 声明链上入口（entrypoint）
//! 2) 根据指令判别字节分发到具体 instruction 处理器
#![allow(unexpected_cfgs)]

use pinocchio::{entrypoint, error::ProgramError, AccountView, Address, ProgramResult};

entrypoint!(process_instruction);

pub mod instructions;
pub use instructions::*;

pub mod state;
pub use state::*;

/// 程序统一入口。
///
/// 指令数据的第 1 个字节作为 discriminator：
/// - 0: initialize
/// - 1: deposit
/// - 2: withdraw
/// - 3: swap
fn process_instruction(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    match instruction_data.split_first() {
        Some((Initialize::DISCRIMINATOR, data)) => {
            Initialize::try_from((data, accounts))?.process(program_id)
        }
        Some((Deposit::DISCRIMINATOR, data)) => {
            Deposit::try_from((data, accounts))?.process(program_id)
        }
        Some((Withdraw::DISCRIMINATOR, data)) => {
            Withdraw::try_from((data, accounts))?.process(program_id)
        }
        Some((Swap::DISCRIMINATOR, data)) => Swap::try_from((data, accounts))?.process(program_id),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}
