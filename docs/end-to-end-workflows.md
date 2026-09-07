# VoiceFlow End-to-End Workflows

## 1. 普通口述：默认自动交付

```mermaid
flowchart LR
  A[用户按快捷键] --> B[捕获音频]
  B --> C[ASR]
  C --> D[本地解析 CleanupIntent]
  D --> E[AI cleanup]
  E -->|成功| F[protected facts 校验]
  E -->|失败/超时/空响应| G[local cleanup]
  F --> H[delivery policy]
  G --> H
  H -->|尝试输入框| I[目标 guard + clipboard + Cmd+V]
  I --> J{焦点 value 可验证?}
  J -->|是| K[done + 3 秒 Undo]
  J -->|否| L[paste_unverified + clipboard 可手动恢复]
  I -->|目标/权限/快捷键失败| M[clipboard]
  H -->|只复制| M
  H -->|仅历史| N[History]
  M --> O[copied]
  K --> N
  L --> N
  O --> N
```

关键语义：`Cmd+V` 发出成功不等于目标控件接收成功。macOS 能读取焦点 Accessibility value 且包含本次文本时才标记 `paste`；其余情况标记 `paste_unverified`，并保留剪贴板内容。

录音手势：默认组合键 tap 切换（再按一次结束，Esc 取消）；也可选 hybrid（短按切换、按住说话）；功能键只能双击。

## 2. 长录音

1. 超过分段阈值后，音频按 chunk 处理；HUD 在 caption chip 中显示 `正在识别 3/8` 这类进度，132×34 药丸仍保持紧凑。
2. 所有 chunk 完成后合并 transcript，再执行一次 cleanup 和 protected-fact 校验。
3. Settings 的“长录音输出”使用 `delivery_policy`：

   - 自动粘贴：优先写入输入框，失败后复制到剪贴板；
   - 写入当前输入框：同样使用 fail-closed target guard，无法安全写入时复制；
   - 复制到剪贴板：跳过键盘注入；
   - 仅保存到历史：不修改当前 App，也不覆盖剪贴板。

4. 任何部分 ASR、AI cleanup 或 delivery 降级都会保留 raw/final、原因和可恢复音频到 History。

当前 ASR 是 batch 上传，不是 WebSocket 流式。prefetch 会把已完成的非 warmup 分块预览写到 HUD（最多约 280 字），不进剪贴板、外部 App 或 History。最终仍走完整 ASR + cleanup 路径。

## 3. 选中文本助手：preview-first

```mermaid
sequenceDiagram
  participant U as 用户
  participant V as VoiceFlow
  participant T as 原目标 App
  U->>T: 选中文字
  U->>V: 按 selected-action hotkey 并口述指令
  V->>T: 临时 Cmd+C，读取后恢复原剪贴板
  V->>V: ASR + explicit selected-text intent + LLM
  V-->>U: 可编辑 preview
  alt 替换原文
    U->>V: 修改结果并点击替换
    V->>T: 重新激活原 PID
    V->>T: 再次验证 App/window/focus/selection
    alt 验证通过
      V->>T: Cmd+V，并执行本地 value best-effort 验证
    else 目标或选区变化
      V->>V: 复制结果，不替换原文
    end
  else 只复制
    U->>V: 点击只复制
    V->>V: 写入剪贴板，不写入目标 App
  else 取消
    U->>V: 点击取消
    V->>V: 清除内存 preview
  end
```

选中文本、语音指令和生成结果默认只存在内存；只有用户主动编辑并保存到 History 时才进入持久化版本链。

## 4. Undo

Undo 只针对最近一次已完成的 paste transaction，生命周期 3 秒。点击后会再次比较 session generation、App/PID、window、browser target 和 focused input；任一目标变化就返回 `stale_target`，绝不发送盲目的 Cmd+Z。连续第二次插入会使前一次 transaction 失效。

## 5. Screen assistant / 看屏幕

默认听写只走辅助功能读到的字（Phase 1），可选本机窗口 OCR（Phase 2，不上传）。**截图进 LLM 只发生在「看屏幕」热键：**

1. 用户自己录了 `screen_action_hotkey`（空着不注册）。
2. 已配置 `vision_provider` / `vision_model`；否则提示 configure a vision model，**不截屏**。
3. 需要屏幕录制 + 辅助功能，否则只打开系统设置。
4. 只截当前锁定窗口一张内存 PNG（长边 ≤1280），不落盘。
5. 用户说话 → ASR → 把图和指令发给用户配置的 vision provider。
6. 预览：缩略图、建议文本、替换 / 只复制 / 取消。目标变了只复制。取消丢掉 PNG。
7. 不自动写入 History。会议录音、Spark、生图仍然不做。

听写松手路径不得调用 Phase 3 的 `capture_for_vision`。
