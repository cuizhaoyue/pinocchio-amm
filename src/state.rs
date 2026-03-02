//! AMM 链上状态定义与读写辅助。
//!
//! 设计目标：
//! - 使用紧凑、可预测的内存布局（`#[repr(C)]`）
//! - 对外提供安全的 load/load_mut 封装，统一做 owner/长度校验
//! - 所有可变字段都通过 setter 写入，集中做约束校验

use core::mem::size_of;

use pinocchio::{
    account::{Ref, RefMut},
    error::ProgramError,
    AccountView, Address,
};

/// AMM 配置账户。
///
/// 注意：
/// - `seed`、`fee` 使用字节数组存储，避免潜在未对齐读写问题。
/// - 所有字段均为固定长度，便于零拷贝访问与长度校验。
#[repr(C)]
pub struct Config {
    /// 当前池状态（见 `AmmState`）。
    state: u8,
    /// 池子 seed（u64 小端）。
    seed: [u8; 8],
    /// 管理员地址；全 0 表示不可变池。
    authority: Address,
    /// 交易对 X 币种 mint。
    mint_x: Address,
    /// 交易对 Y 币种 mint。
    mint_y: Address,
    /// 费率（u16，小端；单位 bps，10000 = 100%）。
    fee: [u8; 2],
    /// config PDA bump。
    config_bump: [u8; 1],
}

/// AMM 生命周期状态。
#[repr(u8)]
pub enum AmmState {
    /// 尚未初始化。
    Uninitialized = 0u8,
    /// 正常运行（可存取、可交易）。
    Initialized = 1u8,
    /// 完全禁用。
    Disabled = 2u8,
    /// 仅允许 withdraw（可用于紧急模式）。
    WithdrawOnly = 3u8,
}

impl Config {
    /// Config 账户固定长度。
    pub const LEN: usize = size_of::<Config>();

    /// 安全只读加载：校验数据长度 + owner。
    ///
    /// 这里的 owner 不是硬编码常量，而是当前运行时传入的 program_id，
    /// 这样同一份程序可在本地测试、devnet、mainnet 使用不同部署地址。
    #[inline(always)]
    pub fn load<'a>(
        account_view: &'a AccountView,
        program_id: &Address,
    ) -> Result<Ref<'a, Self>, ProgramError> {
        if account_view.data_len() != Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        if !account_view.owned_by(program_id) {
            return Err(ProgramError::InvalidAccountOwner);
        }

        Ok(Ref::map(account_view.try_borrow()?, |data| unsafe {
            Self::from_bytes_unchecked(data)
        }))
    }

    /// 非借用检查的只读加载（仍校验长度和 owner）。
    ///
    /// # Safety
    /// 调用方需保证此时不存在冲突的可变借用。
    #[inline(always)]
    pub unsafe fn load_unchecked<'a>(
        account_view: &'a AccountView,
        program_id: &Address,
    ) -> Result<&'a Self, ProgramError> {
        if account_view.data_len() != Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        if !account_view.owned_by(program_id) {
            return Err(ProgramError::InvalidAccountOwner);
        }

        Ok(Self::from_bytes_unchecked(account_view.borrow_unchecked()))
    }

    /// 安全可变加载：校验数据长度 + owner。
    #[inline(always)]
    pub fn load_mut<'a>(
        account_view: &'a AccountView,
        program_id: &Address,
    ) -> Result<RefMut<'a, Self>, ProgramError> {
        if account_view.data_len() != Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        if !account_view.owned_by(program_id) {
            return Err(ProgramError::InvalidAccountOwner);
        }

        Ok(RefMut::map(account_view.try_borrow_mut()?, |data| unsafe {
            Self::from_bytes_unchecked_mut(data)
        }))
    }

    /// 从原始字节零拷贝读取 Config。
    ///
    /// # Safety
    /// 调用方需保证字节布局与 `Config` 一致。
    #[inline(always)]
    pub unsafe fn from_bytes_unchecked(bytes: &[u8]) -> &Self {
        &*(bytes.as_ptr() as *const Config)
    }

    /// 从原始字节零拷贝读取可变 Config。
    ///
    /// # Safety
    /// 调用方需保证字节布局与 `Config` 一致，且无别名可变引用冲突。
    #[inline(always)]
    pub unsafe fn from_bytes_unchecked_mut(bytes: &mut [u8]) -> &mut Self {
        &mut *(bytes.as_mut_ptr() as *mut Config)
    }

    // ===== 读取接口（统一转换） =====

    #[inline(always)]
    pub fn state(&self) -> u8 {
        self.state
    }

    #[inline(always)]
    pub fn seed(&self) -> u64 {
        u64::from_le_bytes(self.seed)
    }

    #[inline(always)]
    pub fn authority(&self) -> &Address {
        &self.authority
    }

    #[inline(always)]
    pub fn mint_x(&self) -> &Address {
        &self.mint_x
    }

    #[inline(always)]
    pub fn mint_y(&self) -> &Address {
        &self.mint_y
    }

    #[inline(always)]
    pub fn fee(&self) -> u16 {
        u16::from_le_bytes(self.fee)
    }

    #[inline(always)]
    pub fn config_bump(&self) -> [u8; 1] {
        self.config_bump
    }

    // ===== 写入接口（集中校验） =====

    /// 设置状态并校验枚举范围。
    #[inline(always)]
    pub fn set_state(&mut self, state: u8) -> Result<(), ProgramError> {
        if state > AmmState::WithdrawOnly as u8 {
            return Err(ProgramError::InvalidAccountData);
        }
        self.state = state;
        Ok(())
    }

    #[inline(always)]
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed.to_le_bytes();
    }

    #[inline(always)]
    pub fn set_authority(&mut self, authority: Address) {
        self.authority = authority;
    }

    #[inline(always)]
    pub fn set_mint_x(&mut self, mint_x: Address) {
        self.mint_x = mint_x;
    }

    #[inline(always)]
    pub fn set_mint_y(&mut self, mint_y: Address) {
        self.mint_y = mint_y;
    }

    /// 设置费率（bps），必须小于 10000。
    #[inline(always)]
    pub fn set_fee(&mut self, fee: u16) -> Result<(), ProgramError> {
        if fee >= 10_000 {
            return Err(ProgramError::InvalidAccountData);
        }
        self.fee = fee.to_le_bytes();
        Ok(())
    }

    #[inline(always)]
    pub fn set_config_bump(&mut self, config_bump: [u8; 1]) {
        self.config_bump = config_bump;
    }

    /// 一次性写入配置。
    ///
    /// 这样可以把“初始化后的最终状态”集中在一个地方，降低部分字段漏写风险。
    #[inline(always)]
    pub fn set_inner(
        &mut self,
        seed: u64,
        authority: Address,
        mint_x: Address,
        mint_y: Address,
        fee: u16,
        config_bump: [u8; 1],
    ) -> Result<(), ProgramError> {
        self.set_state(AmmState::Initialized as u8)?;
        self.set_seed(seed);
        self.set_authority(authority);
        self.set_mint_x(mint_x);
        self.set_mint_y(mint_y);
        self.set_fee(fee)?;
        self.set_config_bump(config_bump);
        Ok(())
    }

    /// 判断是否存在有效 authority（非全 0）。
    #[inline(always)]
    pub fn has_authority(&self) -> Option<Address> {
        // 用 u64 分块比较比逐字节循环更高效。
        let bytes = self.authority();
        let chunks: &[u64; 4] = unsafe { &*(bytes.as_array().as_ptr() as *const [u64; 4]) };
        if chunks.iter().any(|&x| x != 0) {
            // Address 在 0.10 依赖栈里默认不是 Copy，这里显式 clone 返回拥有权。
            Some(self.authority.clone())
        } else {
            None
        }
    }
}
