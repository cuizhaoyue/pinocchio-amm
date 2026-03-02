//! 指令层公共工具函数。
//!
//! 目的：
//! - 消除 `deposit/withdraw/swap` 中重复的账户校验代码
//! - 让每条指令的 `process` 更聚焦在业务流程本身

use pinocchio::{
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address,
};

/// 要求某账户必须是 signer。
#[inline(always)]
pub fn require_signer(account: &AccountView) -> Result<(), ProgramError> {
    if !account.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    Ok(())
}

/// 校验程序账户：地址匹配且可执行。
#[inline(always)]
pub fn require_program(
    program_account: &AccountView,
    expected_program_id: &Address,
) -> Result<(), ProgramError> {
    if program_account.address() != expected_program_id || !program_account.executable() {
        return Err(ProgramError::IncorrectProgramId);
    }
    Ok(())
}

/// 校验一组 `u64` 数值都必须 > 0。
#[inline(always)]
pub fn require_non_zero(values: &[u64]) -> Result<(), ProgramError> {
    if values.iter().any(|v| *v == 0) {
        return Err(ProgramError::InvalidInstructionData);
    }
    Ok(())
}

/// 校验订单未过期（当前链上时间 <= expiration）。
#[inline(always)]
pub fn require_not_expired(expiration: i64) -> Result<(), ProgramError> {
    if Clock::get()?.unix_timestamp > expiration {
        return Err(ProgramError::InvalidArgument);
    }
    Ok(())
}

/// 验证某 vault 是否是 `config + token_program + mint` 对应的 ATA PDA。
#[inline(always)]
pub fn verify_vault_ata(
    account: &AccountView,
    owner: &AccountView,
    token_program: &AccountView,
    mint: &Address,
) -> Result<(), ProgramError> {
    let (expected_vault, _) = Address::find_program_address(
        &[
            owner.address().as_ref(),
            token_program.address().as_ref(),
            mint.as_ref(),
        ],
        &pinocchio_associated_token_account::ID,
    );

    if account.address() != &expected_vault {
        return Err(ProgramError::InvalidAccountData);
    }

    Ok(())
}
