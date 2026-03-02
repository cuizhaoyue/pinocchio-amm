//! deposit 指令：向池子注入 X/Y 流动性并铸造 LP。
//!
//! 核心步骤：
//! 1) 校验账户与订单参数
//! 2) 根据当前池子储备计算应存入的 X/Y 数量
//! 3) 从用户 ATA 转入金库
//! 4) 给用户铸造对应数量 LP

use constant_product_curve::ConstantProduct;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use pinocchio_token::{
    instructions::{MintTo, Transfer},
    state::{Mint, TokenAccount},
};

use crate::{
    instructions::helper::{
        require_non_zero, require_not_expired, require_program, require_signer, verify_vault_ata,
    },
    state::{AmmState, Config},
};

/// deposit 指令账户列表。
///
/// 顺序：
/// 0. user
/// 1. mint_lp
/// 2. vault_x
/// 3. vault_y
/// 4. user_x_ata
/// 5. user_y_ata
/// 6. user_lp_ata
/// 7. config
/// 8. token_program
pub struct DepositAccounts<'a> {
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

impl<'a> TryFrom<&'a [AccountView]> for DepositAccounts<'a> {
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

/// deposit 指令数据。
#[repr(C, packed)]
pub struct DepositInstructionData {
    /// 要铸造的 LP 数量。
    pub amount: u64, // 要铸造的 LP 数量
    /// 用户可接受的 X 最大输入。
    pub max_x: u64, // 用户可接受的 X 最大输入
    /// 用户可接受的 Y 最大输入。
    pub max_y: u64, // 用户可接受的 Y 最大输入
    /// 过期时间（unix timestamp）。
    pub expiration: i64, // 订单过期时间（unix timestamp）
}

impl TryFrom<&[u8]> for DepositInstructionData {
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
            max_x: u64::from_le_bytes(
                data[8..16]
                    .try_into()
                    .map_err(|_| ProgramError::InvalidInstructionData)?,
            ),
            max_y: u64::from_le_bytes(
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

/// deposit 指令处理器。
pub struct Deposit<'a> {
    pub accounts: DepositAccounts<'a>, // deposit 指令所需账户
    pub instruction_data: DepositInstructionData, // deposit 指令参数
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountView])> for Deposit<'a> {
    type Error = ProgramError;

    fn try_from((data, accounts): (&'a [u8], &'a [AccountView])) -> Result<Self, Self::Error> {
        let accounts = DepositAccounts::try_from(accounts)?;
        let instruction_data = DepositInstructionData::try_from(data)?;

        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Deposit<'a> {
    /// deposit 的 discriminator = 1。
    pub const DISCRIMINATOR: &'a u8 = &1;

    pub fn process(&mut self, program_id: &Address) -> ProgramResult {
        // ---------- 通用参数检查 ----------
        require_signer(self.accounts.user)?;
        require_program(self.accounts.token_program, &pinocchio_token::ID)?;
        require_non_zero(&[
            self.instruction_data.amount,
            self.instruction_data.max_x,
            self.instruction_data.max_y,
        ])?;
        require_not_expired(self.instruction_data.expiration)?;

        // ---------- 读取并校验 config ----------
        let config_data = Config::load(self.accounts.config, program_id)?;
        if config_data.state() != AmmState::Initialized as u8 {
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
            // LP mint 的 mint_authority 必须是 config PDA。
            if mint_lp.mint_authority() != Some(self.accounts.config.address()) {
                return Err(ProgramError::InvalidAccountData);
            }
            mint_lp.supply()
        };

        let (vault_x_amount, vault_y_amount) = {
            let vault_x =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.vault_x)? };
            let vault_y =
                unsafe { TokenAccount::from_account_view_unchecked(self.accounts.vault_y)? };

            // 金库 token account 必须由 config 持有，并且 mint 与 config 对应。
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

        // ---------- 计算本次应存入 X/Y ----------
        // 首次注入时，直接采用用户给出的 max_x / max_y。
        let (x, y) = if mint_lp_supply == 0 && vault_x_amount == 0 && vault_y_amount == 0 {
            (self.instruction_data.max_x, self.instruction_data.max_y)
        } else {
            let amounts = ConstantProduct::xy_deposit_amounts_from_l(
                vault_x_amount,
                vault_y_amount,
                mint_lp_supply,
                self.instruction_data.amount,
                6,
            )
            .map_err(|_| ProgramError::InvalidArgument)?;

            (amounts.x, amounts.y)
        };

        // 滑点保护：实际扣款不能超过用户给的上限。
        if !(x <= self.instruction_data.max_x && y <= self.instruction_data.max_y) {
            return Err(ProgramError::InvalidArgument);
        }

        // ---------- 使用 config PDA 作为 LP mint authority ----------
        let seed_binding = config_seed.to_le_bytes();
        let signer_seeds = [
            Seed::from(b"config"),
            Seed::from(&seed_binding),
            Seed::from(config_mint_x.as_ref()),
            Seed::from(config_mint_y.as_ref()),
            Seed::from(&config_bump),
        ];
        let signer = [Signer::from(&signer_seeds)];

        // 用户 -> vault_x
        Transfer {
            from: self.accounts.user_x_ata,
            to: self.accounts.vault_x,
            authority: self.accounts.user,
            amount: x,
        }
        .invoke()?;

        // 用户 -> vault_y
        Transfer {
            from: self.accounts.user_y_ata,
            to: self.accounts.vault_y,
            authority: self.accounts.user,
            amount: y,
        }
        .invoke()?;

        // 铸造 LP 到用户 LP ATA
        MintTo {
            mint: self.accounts.mint_lp,
            account: self.accounts.user_lp_ata,
            mint_authority: self.accounts.config,
            amount: self.instruction_data.amount,
        }
        .invoke_signed(&signer)?;

        Ok(())
    }
}
