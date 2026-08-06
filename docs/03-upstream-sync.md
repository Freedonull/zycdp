# 03 - 上游同步流程

> 本文档定义如何定期把 `mattsse/chromiumoxide`（上游）的更新合并进 zycdp。
> 这是"长期 fork"策略可持续执行的关键流程。

## 一、仓库 remote 配置

zycdp 已配置两个 remote（验证：`git remote -v`）：

| remote | URL | 用途 |
|---|---|---|
| `origin` | `https://github.com/Freedonull/zycdp.git` | 你自己的 fork，push 目标 |
| `upstream` | `https://github.com/ccheshirecat/chaser-oxide.git` | zycdp 的直接前身 |

> ⚠️ **注意**：`upstream` 当前指向的是 `chaser-oxide`（原 fork），不是 `chromiumoxide`。
> - 如果 chaser-oxide 仍在维护，同步它即可（它已合并 chromiumoxide 的更新）。
> - 如果 chaser-oxide 停更，需额外加 `chromiumoxide` 作为二级上游（见文末"二级上游"）。

## 二、定期同步流程（每月或按需）

### 前置检查

```bash
# 1. 确认工作区干净
git status

# 2. 切到 main（或你的主干分支）
git checkout main

# 3. 拉取 upstream 最新
git fetch upstream
```

### 执行合并

```bash
# 合并 upstream/main 到本地 main
git merge upstream/main
```

### 处理冲突（重点）

zycdp 的 stealth 改动深度侵入上游活跃文件，以下文件**极易冲突**：

| 高冲突文件 | zycdp 的改动 | 冲突时如何处理 |
|---|---|---|
| `src/handler/frame.rs:217` | 删除 `Runtime.enable` 调用 + 注释 | 保持 zycdp 的删除（"We do NOT enable Runtime"），丢弃上游重新加回的 `enable_runtime` |
| `src/browser/config.rs:469` | `DEFAULT_ARGS` 从 24 条裁到 19 条 | 保持 zycdp 的裁剪版本，丢弃上游新增的自动化 flag |
| `src/page.rs` | `enable_stealth_mode` 系列私有方法 | 通常 zycdp 新增的私有方法不冲突；若上游重构了同区域，逐方法对照 |
| `src/chaser.rs` / `src/profiles.rs` | zycdp 独有文件 | 几乎不冲突（上游没有这些文件） |

**冲突处理原则**：
1. **stealth 改动优先保留**：任何 `// ZYCDP-STEALTH` / `// chaser-oxide Stealth` 标记的代码段，是 zycdp 的核心价值，不能被上游覆盖。
2. **CDP 协议更新接受上游**：`chromiumoxide_cdp/`（生成代码）和 PDL 文件的更新，直接接受上游。
3. **拿不准时二分验证**：若不确定某个上游 commit 是否破坏 stealth，用 `git log -p <file>` 看该 commit 改了什么。

### 合并后验证（强制）

```bash
# 1. 编译验证
cargo check --lib --examples --tests

# 2. 运行离线 stealth 回归测试（P0-3 完成后）
cargo test --test stealth -- --include-ignored

# 3. 手动跑一次反爬检测站点（可选但推荐）
#    用 examples/stealth_bot.rs 对 bot.sannysoft.com 或 rebrowser-bot-detector
```

**任一验证失败 → 不要 push**，回退合并：
```bash
git merge --abort
# 或合并已 commit：
git reset --hard ORIG_HEAD
```

### 推送

```bash
# 验证全通过后推送到自己的 fork
git push origin main
```

## 三、二级上游（chromiumoxide）的同步

如果 `chaser-oxide`（你的 upstream）长期停更，而你又需要 chromiumoxide 的 bugfix/CDP 更新，需要直接同步 chromiumoxide。

### 添加二级上游

```bash
git remote add chromiumoxide https://github.com/mattsse/chromiumoxide.git
git fetch chromiumoxide
```

### 合并二级上游（更易冲突）

```bash
git merge chromiumoxide/main
```

**风险更高**：chaser-oxide 已经做过一轮 stealth 改造，直接合并 chromiumoxide 会跳过 chaser-oxide 的中间层，可能产生更复杂的冲突。建议：
- 优先等 chaser-oxide 合并后，你同步 chaser-oxide（风险更低）
- 只有在 chaser-oxide 明确停更且你需要紧急 bugfix 时，才直接合并 chromiumoxide

## 四、同步决策表

| 情况 | 动作 |
|---|---|
| chaser-oxide 有新 commit | 同步 upstream（chaser-oxide） |
| chaser-oxide 停更 + chromiumoxide 有重要更新 | 同步二级上游（chromiumoxide） |
| 两者都没动 | 无需同步 |
| 同步后 stealth 测试 fail | 回退，二分定位问题 commit，单独 cherry-pick 安全的改动 |

## 五、版本标记

每次成功同步上游后，建议打 tag 便于回溯：

```bash
git tag -a v0.3.0-sync-$(date +%Y%m%d) -m "同步上游至 $(git rev-parse --short upstream/main)"
git push origin --tags
```

## 六、检查清单（每次同步走一遍）

- [ ] 工作区干净
- [ ] `git fetch upstream` 成功
- [ ] `git merge upstream/main` 完成（或冲突已手动 resolve）
- [ ] `cargo check --lib --examples --tests` 全通过
- [ ] stealth 回归测试全通过
- [ ] stealth 改动（`Runtime.enable` 删除、`DEFAULT_ARGS` 裁剪）仍存在
- [ ] `git push origin main`
- [ ] 打 tag 记录同步点
