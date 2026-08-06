# zycdp 改进工作文档

本目录是 zycdp（fork 自 `chaser-oxide`，后者 fork 自 `chromiumoxide`）的开发改进文档。

面向 zycdp 的长期维护者，记录项目的技术原理、已知缺陷、改进路线和协作流程。

## 文档索引

| 文档 | 内容 | 何时阅读 |
|---|---|---|
| [01-architecture.md](./01-architecture.md) | 项目架构与 stealth 技术原理 | 想理解"为什么这样改""对抗了什么检测" |
| [02-improvement-roadmap.md](./02-improvement-roadmap.md) | 改进路线图（P0/P1/P2 分级） | 规划下一个开发周期 |
| [03-upstream-sync.md](./03-upstream-sync.md) | 同步上游 chromiumoxide 的流程 | 每次定期 merge 上游前 |
| [04-usage-guide.md](./04-usage-guide.md) | 使用指南（含指纹浏览器连接） | 集成到业务项目时 |
| [05-defects-baseline.md](./05-defects-baseline.md) | 已知缺陷与待验证项 baseline | 改动前对照，避免回归 |

## 快速上手

新接手本项目时，建议按以下顺序阅读：

1. **先读 [01-architecture.md](./01-architecture.md)**：理解 zycdp 的核心价值（stealth 内核）和它相对上游 chromiumoxide 的改动边界。
2. **再读 [05-defects-baseline.md](./05-defects-baseline.md)**：了解当前代码中已确认的缺陷和未验证的声明，避免踩坑或重复劳动。
3. **开发前读 [02-improvement-roadmap.md](./02-improvement-roadmap.md)**：选择优先级最高的改进项动手。
4. **集成时读 [04-usage-guide.md](./04-usage-guide.md)**：了解依赖配置、指纹浏览器连接、API 红线。
5. **merge 上游前读 [03-upstream-sync.md](./03-upstream-sync.md)**：按流程操作，避免冲突和回归。

## 文档维护原则

- **代码改动必须同步更新文档**：尤其是新增/修改 stealth 对抗项时，更新 `01-architecture.md` 对应表格。
- **缺陷修复必须更新 baseline**：`05-defects-baseline.md` 的每一条缺陷在修复后标记 ✅ 并附 commit hash。
- **决策有据**：所有"为什么这样做"的结论，文档里必须附代码位置（`file:line`）或外部来源链接。

## 术语约定

- **上游（upstream）**：指 `mattsse/chromiumoxide`，CDP 客户端的原始项目。
- **原 fork（origin fork）**：指 `ccheshirecat/chaser-oxide`，本项目的直接前身，已做了 stealth 改造。
- **zycdp**：本仓库，在原 fork 基础上继续优化（Freedonull/zycdp）。
- **stealth**：反检测/隐身，指降低自动化浏览器被反爬系统识别为 bot 的能力。
