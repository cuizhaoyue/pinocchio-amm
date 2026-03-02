//! initialize 指令：创建并初始化 AMM 的核心状态。
//!
//! 本指令完成两件关键事情：
//! 1) 创建并写入 `config`（AMM 配置账户）
//! 2) 创建并初始化 `mint_lp`（流动性份额代币）

use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use pinocchio_system::create_account_with_minimum_balance_signed;
use pinocchio_token::{instructions::InitializeMint2, state::Mint};

use crate::{
    instructions::helper::{require_program, require_signer},
    state::Config,
};

/// initialize 指令账户列表。
///
/// 顺序必须与前端传参严格一致：
/// 0. initializer
/// 1. mint_lp
/// 2. config
/// 3. system_program
/// 4. token_program
pub struct InitializeAccounts<'a> {
    pub initializer: &'a AccountView, // 初始化者（签名并支付创建账户费用）
    pub mint_lp: &'a AccountView,     // LP mint PDA 账户
    pub config: &'a AccountView,      // AMM 配置 PDA 账户
    pub system_program: &'a AccountView, // System Program 账户
    pub token_program: &'a AccountView, // Token Program 账户
}

impl<'a> TryFrom<&'a [AccountView]> for InitializeAccounts<'a> {
    type Error = ProgramError;

    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        if accounts.len() < 5 {
            return Err(ProgramError::NotEnoughAccountKeys);
        }

        Ok(Self {
            initializer: &accounts[0],
            mint_lp: &accounts[1],
            config: &accounts[2],
            system_program: &accounts[3],
            token_program: &accounts[4],
        })
    }
}

/// initialize 指令数据。
///
/// 与 Blueshift 挑战保持一致：
/// - authority 可选：若不传，则按 32 字节全 0 处理（不可变池）
#[repr(C, packed)]
pub struct InitializeInstructionData {
    pub seed: u64,            // 池子种子（用于派生 config PDA）
    pub fee: u16,             // 交易手续费参数
    pub mint_x: [u8; 32],     // X 币种 mint 地址
    pub mint_y: [u8; 32],     // Y 币种 mint 地址
    pub config_bump: [u8; 1], // config PDA 的 bump
    pub lp_bump: [u8; 1],     // mint_lp PDA 的 bump
    pub authority: [u8; 32],  // AMM authority 地址（全 0 表示无 authority）
}

impl TryFrom<&[u8]> for InitializeInstructionData {
    type Error = ProgramError;

    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        const DATA_LEN_WITH_AUTHORITY: usize = 8 + 2 + 32 + 32 + 1 + 1 + 32;
        const DATA_LEN: usize = DATA_LEN_WITH_AUTHORITY - 32;

        match data.len() {
            // 完整格式：直接按固定偏移解析
            DATA_LEN_WITH_AUTHORITY => {
                let seed = u64::from_le_bytes(
                    data[0..8]
                        .try_into()
                        .map_err(|_| ProgramError::InvalidInstructionData)?,
                );
                let fee = u16::from_le_bytes(
                    data[8..10]
                        .try_into()
                        .map_err(|_| ProgramError::InvalidInstructionData)?,
                );

                let mut mint_x = [0u8; 32];
                mint_x.copy_from_slice(&data[10..42]);

                let mut mint_y = [0u8; 32];
                mint_y.copy_from_slice(&data[42..74]);

                let mut config_bump = [0u8; 1];
                config_bump.copy_from_slice(&data[74..75]);

                let mut lp_bump = [0u8; 1];
                lp_bump.copy_from_slice(&data[75..76]);

                let mut authority = [0u8; 32];
                authority.copy_from_slice(&data[76..108]);

                Ok(Self {
                    seed,
                    fee,
                    mint_x,
                    mint_y,
                    config_bump,
                    lp_bump,
                    authority,
                })
            }
            // 缺省 authority：补全为全 0
            DATA_LEN => {
                let seed = u64::from_le_bytes(
                    data[0..8]
                        .try_into()
                        .map_err(|_| ProgramError::InvalidInstructionData)?,
                );
                let fee = u16::from_le_bytes(
                    data[8..10]
                        .try_into()
                        .map_err(|_| ProgramError::InvalidInstructionData)?,
                );

                let mut mint_x = [0u8; 32];
                mint_x.copy_from_slice(&data[10..42]);

                let mut mint_y = [0u8; 32];
                mint_y.copy_from_slice(&data[42..74]);

                let mut config_bump = [0u8; 1];
                config_bump.copy_from_slice(&data[74..75]);

                let mut lp_bump = [0u8; 1];
                lp_bump.copy_from_slice(&data[75..76]);

                Ok(Self {
                    seed,
                    fee,
                    mint_x,
                    mint_y,
                    config_bump,
                    lp_bump,
                    authority: [0u8; 32],
                })
            }
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }
}

/// initialize 指令处理器。
pub struct Initialize<'a> {
    pub accounts: InitializeAccounts<'a>, // 初始化指令所需账户
    pub instruction_data: InitializeInstructionData, // 初始化指令参数
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountView])> for Initialize<'a> {
    type Error = ProgramError;

    fn try_from((data, accounts): (&'a [u8], &'a [AccountView])) -> Result<Self, Self::Error> {
        let accounts = InitializeAccounts::try_from(accounts)?;
        let instruction_data = InitializeInstructionData::try_from(data)?;

        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Initialize<'a> {
    /// initialize 的 discriminator = 0。
    pub const DISCRIMINATOR: &'a u8 = &0;

    /// 执行 initialize 主流程。
    pub fn process(&mut self, program_id: &Address) -> ProgramResult {
        // ---------- 基础账户约束 ----------
        require_signer(self.accounts.initializer)?;
        require_program(self.accounts.system_program, &pinocchio_system::ID)?;
        require_program(self.accounts.token_program, &pinocchio_token::ID)?;

        // X/Y 不允许是同一个 mint。
        if self.instruction_data.mint_x == self.instruction_data.mint_y {
            return Err(ProgramError::InvalidInstructionData);
        }

        // ---------- 创建 config PDA ----------
        let seed_binding = self.instruction_data.seed.to_le_bytes();
        let config_seeds = [
            Seed::from(b"config"),
            Seed::from(&seed_binding),
            Seed::from(&self.instruction_data.mint_x),
            Seed::from(&self.instruction_data.mint_y),
            Seed::from(&self.instruction_data.config_bump),
        ];
        let config_signers = [Signer::from(&config_seeds)];

        create_account_with_minimum_balance_signed(
            self.accounts.config,
            Config::LEN,
            program_id,
            self.accounts.initializer,
            None,
            &config_signers,
        )?;

        // 写入 config 关键参数。
        {
            let mut config = Config::load_mut(self.accounts.config, program_id)?;
            config.set_inner(
                self.instruction_data.seed,
                Address::new_from_array(self.instruction_data.authority),
                Address::new_from_array(self.instruction_data.mint_x),
                Address::new_from_array(self.instruction_data.mint_y),
                self.instruction_data.fee,
                self.instruction_data.config_bump,
            )?;
        }

        // ---------- 创建 mint_lp PDA 并初始化 ----------
        let mint_lp_seeds = [
            Seed::from(b"mint_lp"),
            Seed::from(self.accounts.config.address().as_ref()),
            Seed::from(&self.instruction_data.lp_bump),
        ];
        let mint_lp_signers = [Signer::from(&mint_lp_seeds)];

        create_account_with_minimum_balance_signed(
            self.accounts.mint_lp,
            Mint::LEN,
            &pinocchio_token::ID,
            self.accounts.initializer,
            None,
            &mint_lp_signers,
        )?;

        // LP mint 的铸币权限设为 config PDA（池子自己控制 LP 发行）。
        InitializeMint2 {
            mint: self.accounts.mint_lp,
            decimals: 6,
            mint_authority: self.accounts.config.address(),
            freeze_authority: None,
        }
        .invoke()?;

        Ok(())
    }
}
