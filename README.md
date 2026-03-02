# Pinocchio AMM

使用 [Pinocchio](https://github.com/kevinheavey/pinocchio) 框架开发的 Solana AMM（自动做市商）合约。

## 项目简介

这是一个基于常数乘积曲线（Constant Product Curve）的 AMM 实现，使用 Pinocchio 框架以获得更高效的编译时间和更小的程序体积。该项目是 [Blueshift Pinocchio AMM 挑战](https://learn.blueshift.gg/zh-CN/challenges/pinocchio-amm)的实现。

## 核心功能

- **Initialize (初始化)**: 创建 AMM 池配置和 LP 代币
- **Deposit (存入流动性)**: 向池子存入双代币并获得 LP 代币
- **Withdraw (提取流动性)**: 燃烧 LP 代币并按比例提取双代币
- **Swap (交易)**: 在两个代币之间进行兑换

## 架构设计

### 状态管理 (`state.rs`)

- **Config**: AMM 配置账户，存储池子状态、费率、管理员权限等信息
- **AmmState**: 池子生命周期状态（未初始化、正常运行、禁用、仅提现）

### 指令模块 (`instructions/`)

| 指令 | Discriminator | 功能描述 |
|------|---------------|----------|
| Initialize | 0 | 创建并初始化 AMM 池 |
| Deposit | 1 | 向池子添加流动性 |
| Withdraw | 2 | 从池子移除流动性 |
| Swap | 3 | 执行代币兑换 |

## 依赖项

```toml
constant-product-curve = "0.1.0"  # 常数乘积曲线计算
pinocchio = "0.10.2"               # Pinocchio 核心库
pinocchio-token = "0.5.0"          # Token Program CPI
pinocchio-system = "0.5.0"         # System Program CPI
pinocchio-associated-token-account = "0.3.0"  # ATA CPI
solana-address = "2.2.0"           # 地址类型
solana-program-log = "1.1.0"       # 日志输出
```

## 构建

```bash
cargo build-bpf
```

## 账户结构

### Initialize 指令
1. `initializer` - 初始化者（签名者）
2. `mint_lp` - LP 代币 mint (PDA)
3. `config` - AMM 配置账户 (PDA)
4. `system_program` - System Program
5. `token_program` - Token Program

### Swap 指令
1. `user` - 用户签名者账户
2. `user_x_ata` - 用户 X 代币 ATA
3. `user_y_ata` - 用户 Y 代币 ATA
4. `vault_x` - 池子 X 金库 ATA
5. `vault_y` - 池子 Y 金库 ATA
6. `config` - AMM 配置账户
7. `token_program` - Token Program

## PDA 派生

```rust
// Config PDA
["config", seed, mint_x, mint_y]

// LP Mint PDA
["mint_lp", config]
```

## 安全特性

- 严格的账户所有权校验
- 滑点保护（通过 `min` 参数）
- 订单过期时间检查
- 金库 ATA 地址验证
- 费率上限限制（< 100%）
- 状态机控制（Initialized/Disabled/WithdrawOnly）

## 许可证

MIT
