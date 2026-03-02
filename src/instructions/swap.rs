//! swap 指令：在池子中执行 X<->Y 兑换。
//!
//! 核心步骤：
//! 1) 校验参数与账户
//! 2) 根据常数乘积曲线计算可得输出（含 fee）
//! 3) 用户先转入输入币种，再由 config PDA 从 vault 转出输出币种

use constant_product_curve::{ConstantProduct, LiquidityPair};
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use pinocchio_token::{instructions::Transfer, state::TokenAccount};

use crate::{
    instructions::helper::{
        require_non_zero, require_not_expired, require_program, require_signer, verify_vault_ata,
    },
    state::{AmmState, Config},
};

/// swap 指令账户列表。
pub struct SwapAccounts<'a> {
    pub user: &'a AccountView,          // 用户签名者账户
    pub user_x_ata: &'a AccountView,    // 用户 X ATA
    pub user_y_ata: &'a AccountView,    // 用户 Y ATA
    pub vault_x: &'a AccountView,       // 池子 X 金库 ATA
    pub vault_y: &'a AccountView,       // 池子 Y 金库 ATA
    pub config: &'a AccountView,        // AMM 配置账户
    pub token_program: &'a AccountView, // Token Program 账户
}

impl<'a> TryFrom<&'a [AccountView]> for SwapAccounts<'a> {
    type Error = ProgramError;

    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        if accounts.len() < 7 {
            return Err(ProgramError::NotEnoughAccountKeys);
        }

        Ok(Self {
            user: &accounts[0],
            user_x_ata: &accounts[1],
            user_y_ata: &accounts[2],
            vault_x: &accounts[3],
            vault_y: &accounts[4],
            config: &accounts[5],
            token_program: &accounts[6],
        })
    }
}

/// swap 指令数据。
#[repr(C, packed)]
pub struct SwapInstructionData {
    /// true: X->Y；false: Y->X。
    pub is_x: bool, // 兑换方向：true 为 X->Y，false 为 Y->X
    /// 用户输入数量。
    pub amount: u64, // 用户输入数量
    /// 用户可接受的最小输出（滑点保护）。
    pub min: u64, // 用户可接受的最小输出（滑点保护）
    /// 过期时间（unix timestamp）。
    pub expiration: i64, // 订单过期时间（unix timestamp）
}

impl TryFrom<&[u8]> for SwapInstructionData {
    type Error = ProgramError;

    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        if data.len() != 25 {
            return Err(ProgramError::InvalidInstructionData);
        }

        let is_x = match data[0] {
            0 => false,
            1 => true,
            _ => return Err(ProgramError::InvalidInstructionData),
        };

        Ok(Self {
            is_x,
            amount: u64::from_le_bytes(
                data[1..9]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
            min: u64::from_le_bytes(
                data[9..17]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
            expiration: i64::from_le_bytes(
                data[17..25]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
        })
    }
}

/// swap 指令处理器。
pub struct Swap<'a> {
    pub accounts: SwapAccounts<'a>,            // swap 指令所需账户
    pub instruction_data: SwapInstructionData, // swap 指令参数
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountView])> for Swap<'a> {
    type Error = ProgramError;

    fn try_from((data, accounts): (&'a [u8], &'a [AccountView])) -> Result<Self, Self::Error> {
        let accounts = SwapAccounts::try_from(accounts)?;
        let instruction_data = SwapInstructionData::try_from(data)?;

        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Swap<'a> {
    /// swap 的 discriminator = 3。
    pub const DISCRIMINATOR: &'a u8 = &3;

    pub fn process(&mut self, program_id: &Address) -> ProgramResult {
        // ---------- 通用参数检查 ----------
        require_signer(self.accounts.user)?;
        require_program(self.accounts.token_program, &pinocchio_token::ID)?;
        require_non_zero(&[self.instruction_data.amount, self.instruction_data.min])?;
        require_not_expired(self.instruction_data.expiration)?;

        // ---------- 读取并校验 config ----------
        let config_data = Config::load(self.accounts.config, program_id)?;
        if config_data.state() != AmmState::Initialized as u8 {
            return Err(ProgramError::InvalidAccountData);
        }

        let config_seed = config_data.seed();
        let config_mint_x = config_data.mint_x().clone();
        let config_mint_y = config_data.mint_y().clone();
        let config_fee = config_data.fee();
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

        // ---------- 读取金库余额并做账户一致性校验 ----------
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

        // 用户 ATA 必须属于 user，且 mint 与 config 对应。
        {
            let user_x =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.user_x_ata)? };
            let user_y =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.user_y_ata)? };

            if user_x.owner() != self.accounts.user.address()
                || user_y.owner() != self.accounts.user.address()
                || user_x.mint() != &config_mint_x
                || user_y.mint() != &config_mint_y
            {
                return Err(ProgramError::InvalidAccountData);
            }
        }

        // ---------- 常数乘积曲线计算 ----------
        // l 参数在 swap 场景不重要，沿用指南实现传 vault_x_amount。
        let mut curve = ConstantProduct::init(
            vault_x_amount,
            vault_y_amount,
            vault_x_amount,
            config_fee,
            None,
        )
        .map_err(|_| ProgramError::Custom(1))?;

        let pair = if self.instruction_data.is_x {
            LiquidityPair::X
        } else {
            LiquidityPair::Y
        };

        let swap_result = curve
            .swap(
                pair,
                self.instruction_data.amount,
                self.instruction_data.min,
            )
            .map_err(|_| ProgramError::Custom(1))?;

        // 结果保护，避免无意义兑换。
        if swap_result.deposit == 0 || swap_result.withdraw == 0 {
            return Err(ProgramError::InvalidArgument);
        }

        // ---------- 组装 config PDA 签名种子 ----------
        let seed_binding = config_seed.to_le_bytes();
        let signer_seeds = [
            Seed::from(b"config"),
            Seed::from(&seed_binding),
            Seed::from(config_mint_x.as_ref()),
            Seed::from(config_mint_y.as_ref()),
            Seed::from(&config_bump),
        ];
        let signer = [Signer::from(&signer_seeds)];

        // ---------- 先转入用户输入，再由 vault 转出输出 ----------
        if self.instruction_data.is_x {
            // X -> Y：用户把 X 转入 vault_x
            Transfer {
                from: self.accounts.user_x_ata,
                to: self.accounts.vault_x,
                authority: self.accounts.user,
                amount: swap_result.deposit,
            }
            .invoke()?;

            // 池子从 vault_y 转出 Y 给用户
            Transfer {
                from: self.accounts.vault_y,
                to: self.accounts.user_y_ata,
                authority: self.accounts.config,
                amount: swap_result.withdraw,
            }
            .invoke_signed(&signer)?;
        } else {
            // Y -> X：用户把 Y 转入 vault_y
            Transfer {
                from: self.accounts.user_y_ata,
                to: self.accounts.vault_y,
                authority: self.accounts.user,
                amount: swap_result.deposit,
            }
            .invoke()?;

            // 池子从 vault_x 转出 X 给用户
            Transfer {
                from: self.accounts.vault_x,
                to: self.accounts.user_x_ata,
                authority: self.accounts.config,
                amount: swap_result.withdraw,
            }
            .invoke_signed(&signer)?;
        }

        Ok(())
    }
}
