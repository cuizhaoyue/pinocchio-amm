//! withdraw 指令：销毁 LP，并按份额从池子取回 X/Y。
//!
//! 核心步骤：
//! 1) 校验账户与参数
//! 2) 根据池子储备计算可取回的 X/Y
//! 3) 从 vault 转账给用户
//! 4) 销毁用户 LP

use constant_product_curve::ConstantProduct;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use pinocchio_token::{
    instructions::{Burn, Transfer},
    state::{Mint, TokenAccount},
};

use crate::{
    instructions::helper::{
        require_non_zero, require_not_expired, require_program, require_signer, verify_vault_ata,
    },
    state::{AmmState, Config},
};

/// withdraw 指令账户列表（顺序与 deposit 一致）。
pub struct WithdrawAccounts<'a> {
    pub user: &'a AccountView,          // 用户签名者账户
    pub mint_lp: &'a AccountView,       // LP mint 账户
    pub vault_x: &'a AccountView,       // 池子 X 金库 ATA
    pub vault_y: &'a AccountView,       // 池子 Y 金库 ATA
    pub user_x_ata: &'a AccountView,    // 用户 X ATA
    pub user_y_ata: &'a AccountView,    // 用户 Y ATA
    pub user_lp_ata: &'a AccountView,   // 用户 LP ATA
    pub config: &'a AccountView,        // AMM 配置账户
    pub token_program: &'a AccountView, // Token Program 账户
}

impl<'a> TryFrom<&'a [AccountView]> for WithdrawAccounts<'a> {
    type Error = ProgramError;

    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        if accounts.len() < 9 {
            return Err(ProgramError::NotEnoughAccountKeys);
        }

        Ok(Self {
            user: &accounts[0],
            mint_lp: &accounts[1],
            vault_x: &accounts[2],
            vault_y: &accounts[3],
            user_x_ata: &accounts[4],
            user_y_ata: &accounts[5],
            user_lp_ata: &accounts[6],
            config: &accounts[7],
            token_program: &accounts[8],
        })
    }
}

/// withdraw 指令数据。
#[repr(C, packed)]
pub struct WithdrawInstructionData {
    /// 要销毁的 LP 数量。
    pub amount: u64, // 要销毁的 LP 数量
    /// 用户可接受的 X 最小输出。
    pub min_x: u64, // 用户可接受的 X 最小输出
    /// 用户可接受的 Y 最小输出。
    pub min_y: u64, // 用户可接受的 Y 最小输出
    /// 过期时间（unix timestamp）。
    pub expiration: i64, // 订单过期时间（unix timestamp）
}

impl TryFrom<&[u8]> for WithdrawInstructionData {
    type Error = ProgramError;

    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        if data.len() != 32 {
            return Err(ProgramError::InvalidInstructionData);
        }

        Ok(Self {
            amount: u64::from_le_bytes(
                data[0..8]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
            min_x: u64::from_le_bytes(
                data[8..16]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
            min_y: u64::from_le_bytes(
                data[16..24]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
            expiration: i64::from_le_bytes(
                data[24..32]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
        })
    }
}

/// withdraw 指令处理器。
pub struct Withdraw<'a> {
    pub accounts: WithdrawAccounts<'a>, // withdraw 指令所需账户
    pub instruction_data: WithdrawInstructionData, // withdraw 指令参数
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountView])> for Withdraw<'a> {
    type Error = ProgramError;

    fn try_from((data, accounts): (&'a [u8], &'a [AccountView])) -> Result<Self, Self::Error> {
        let accounts = WithdrawAccounts::try_from(accounts)?;
        let instruction_data = WithdrawInstructionData::try_from(data)?;

        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Withdraw<'a> {
    /// withdraw 的 discriminator = 2。
    pub const DISCRIMINATOR: &'a u8 = &2;

    pub fn process(&mut self, program_id: &Address) -> ProgramResult {
        // ---------- 通用参数检查 ----------
        require_signer(self.accounts.user)?;
        require_program(self.accounts.token_program, &pinocchio_token::ID)?;
        require_non_zero(&[
            self.instruction_data.amount,
            self.instruction_data.min_x,
            self.instruction_data.min_y,
        ])?;
        require_not_expired(self.instruction_data.expiration)?;

        // ---------- 读取并校验 config ----------
        let config_data = Config::load(self.accounts.config, program_id)?;
        // 与指南一致：仅 Disabled 禁止 withdraw；WithdrawOnly 可 withdraw。
        if config_data.state() == AmmState::Disabled as u8 {
            return Err(ProgramError::InvalidAccountData);
        }

        let config_seed = config_data.seed();
        let config_mint_x = config_data.mint_x().clone();
        let config_mint_y = config_data.mint_y().clone();
        let config_bump = config_data.config_bump();
        drop(config_data);

        // ---------- 校验 vault ATA 地址正确 ----------
        verify_vault_ata(
            self.accounts.vault_x,
            self.accounts.config,
            self.accounts.token_program,
            &config_mint_x,
        )?;
        verify_vault_ata(
            self.accounts.vault_y,
            self.accounts.config,
            self.accounts.token_program,
            &config_mint_y,
        )?;

        // ---------- 读取 LP mint 与金库余额 ----------
        let mint_lp_supply = {
            let mint_lp = unsafe { Mint::from_account_view_unchecked(self.accounts.mint_lp)? };
            if mint_lp.mint_authority() != Some(self.accounts.config.address()) {
                return Err(ProgramError::InvalidAccountData);
            }
            mint_lp.supply()
        };

        // 提取数量不能超过总 LP。
        if self.instruction_data.amount > mint_lp_supply {
            return Err(ProgramError::InsufficientFunds);
        }

        let (vault_x_amount, vault_y_amount) = {
            let vault_x =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.vault_x)? };
            let vault_y =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.vault_y)? };

            if vault_x.owner() != self.accounts.config.address()
                || vault_y.owner() != self.accounts.config.address()
                || vault_x.mint() != &config_mint_x
                || vault_y.mint() != &config_mint_y
            {
                return Err(ProgramError::InvalidAccountData);
            }

            (vault_x.amount(), vault_y.amount())
        };

        // ---------- 校验用户 ATA 合法性 ----------
        {
            let user_x =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.user_x_ata)? };
            let user_y =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.user_y_ata)? };
            let user_lp =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.user_lp_ata)? };

            if user_x.owner() != self.accounts.user.address()
                || user_y.owner() != self.accounts.user.address()
                || user_lp.owner() != self.accounts.user.address()
                || user_x.mint() != &config_mint_x
                || user_y.mint() != &config_mint_y
                || user_lp.mint() != self.accounts.mint_lp.address()
            {
                return Err(ProgramError::InvalidAccountData);
            }
        }

        // ---------- 计算本次应提取 X/Y ----------
        // 如果销毁的是全部 LP，则直接提走全部储备。
        let (x, y) = if mint_lp_supply == self.instruction_data.amount {
            (vault_x_amount, vault_y_amount)
        } else {
            let amounts = ConstantProduct::xy_withdraw_amounts_from_l(
                vault_x_amount,
                vault_y_amount,
                mint_lp_supply,
                self.instruction_data.amount,
                6,
            )
            .map_err(|_| ProgramError::InvalidArgument)?;

            (amounts.x, amounts.y)
        };

        // 滑点保护：实际输出不能低于用户下限。
        if !(x >= self.instruction_data.min_x && y >= self.instruction_data.min_y) {
            return Err(ProgramError::InvalidArgument);
        }

        // ---------- 使用 config PDA 签名，从 vault 转账给用户 ----------
        let seed_binding = config_seed.to_le_bytes();
        let signer_seeds = [
            Seed::from(b"config"),
            Seed::from(&seed_binding),
            Seed::from(config_mint_x.as_ref()),
            Seed::from(config_mint_y.as_ref()),
            Seed::from(&config_bump),
        ];
        let signer = [Signer::from(&signer_seeds)];

        Transfer {
            from: self.accounts.vault_x,
            to: self.accounts.user_x_ata,
            authority: self.accounts.config,
            amount: x,
        }
        .invoke_signed(&signer)?;

        Transfer {
            from: self.accounts.vault_y,
            to: self.accounts.user_y_ata,
            authority: self.accounts.config,
            amount: y,
        }
        .invoke_signed(&signer)?;

        // 最后销毁用户 LP（由用户签名）。
        Burn {
            account: self.accounts.user_lp_ata,
            mint: self.accounts.mint_lp,
            authority: self.accounts.user,
            amount: self.instruction_data.amount,
        }
        .invoke()?;

        Ok(())
    }
}
