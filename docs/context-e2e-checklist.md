# VoiceFlow Context & Delivery E2E Verification Checklist

更新时间：2026-09-26（当前边界见 §18；自动化数字以本次测试为准）

这份 checklist 用于验证 VoiceFlow 的全部上下文来源、Prompt policy、目标保护、交付方式和 History 结果。它覆盖自动化测试和真实 macOS 外部 App 测试。

## 1. 当前实现边界

| Context / capability | 当前状态 | 验收方式 |
| --- | --- | --- |
| Native App context | 已实现；真实 App 验收待做 | resolver fixtures + §18 native matrix |
| Window context | 已实现 | target guard + 真实多窗口切换 |
| Focused input context | 已实现；外部 App 字段验收待做 | AX target guard + positive field cues；IDE/GitHub app-aware rules；§18 |
| Browser domain context | 已实现；真实 Browser E2E 待做 | browser resolver + §18 native matrix |
| Active Browser Tab context | 已实现 | tab token/window guard + 真实 Tab 切换 |
| User App mapping | 已实现 | Settings + resolver + 真实 App |
| Writing mode | 已实现 | Prompt policy + cleanup corpus |
| Manual context override | 已实现 | policy override + target guard |
| Selected text context | 已实现 | preview-first + 真实选区 |
| Clipboard safety context | 已实现 | clipboard sentinel + cancellation/fallback |
| Text / image context | 已实现（逐 App grants + 有界 AX / 本机 OCR / 云端回退 + 独立看屏幕热键） | 新 grants 默认 off；自动云端图像需具体 App + 独立文字 permission；§18 |

自动窗口图像可在具体 App 规则明确允许 cloud vision、provider text use 和屏幕录制权限后，作为 AX / OCR 不足时的一图回退。独立看屏幕热键仍是单独的用户动作；两条路径都不会截整屏。区域框选仍未做（§18.1 保持未实现）。会议录音、Spark、生图仍然不做。

IDE、GitHub issue/review 和搜索框只按当前 focused editable field 的正向 AX 线索分类。缺少字段线索时保留 `Unknown` 或通用分类；窗口标题、应用名或网站域名不能单独把任意字段改成 chat / prompt。

## 2. 状态和证据约定

每个测试项都必须记录：

- [ ] Test ID
- [ ] 日期和 macOS 版本
- [ ] VoiceFlow commit / 工作区版本
- [ ] 目标 App、Window、Browser Tab
- [ ] Microphone / Accessibility / Browser Automation 权限状态
- [ ] 当前 delivery policy
- [ ] 当前 App mapping / writing mode / manual override
- [ ] 原始语音或选中文本
- [ ] 实际 final text
- [ ] History 中的 status、cleanup_status、fallback_reason、delivery_method
- [ ] Clipboard 变化
- [ ] 目标输入框变化
- [ ] 截图、录屏或事件 payload 证据（如适用）
- [ ] Pass / Fail
- [ ] 如果失败，记录复现步骤和日志，不得只写“偶发失败”

状态含义：

- PASS：实际结果符合预期，且有证据。
- FAIL：结果错误、目标不安全、状态误报或缺少预期 fallback。
- BLOCKED：能力尚未实现或缺少明确外部权限。
- N/A：该测试不适用于当前目标，但必须写明原因。

## 3. 每次代码变更的验证协议

任何 Rust、React、Prompt、context、delivery、History 或测试变更，都必须按以下顺序验证。

### 3.1 变更前基线

- [ ] cargo test -q --manifest-path src-tauri/Cargo.toml
- [ ] npm test -- --run
- [ ] npm run lint
- [ ] npm run build
- [ ] git diff --check
- [ ] 记录变更前测试数量和失败数量

### 3.2 变更后最小验证

- [ ] 运行与改动模块对应的 Rust tests
- [ ] 运行与改动组件对应的 frontend tests
- [ ] 改动 Prompt / context / delivery 时运行对应 corpus 或 resolver tests
- [ ] 改动 paste / target guard / Undo 时运行全部 paste/lib safety tests
- [ ] 改动 History / store 时运行 migration/revision/history tests

### 3.3 变更后完整验证

- [ ] cargo fmt --manifest-path src-tauri/Cargo.toml --all
- [ ] cargo test -q --manifest-path src-tauri/Cargo.toml
- [ ] cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --all-features -- -D warnings
- [ ] npm test -- --run
- [ ] npm run lint
- [ ] npm run build
- [ ] git diff --check
- [ ] 更新本文件中的测试证据或对应 acceptance 文档

### 3.4 真实 macOS 回归触发条件

只要变更涉及以下任一项，就必须重新执行对应真实 macOS 测试：

- [ ] Accessibility / AX
- [ ] Browser Automation
- [ ] target guard
- [ ] Clipboard
- [ ] Cmd+V / Cmd+Z
- [ ] Window / Tab fingerprint
- [ ] selected text
- [ ] HUD state
- [ ] delivery policy

自动化测试不能替代真实 App 输入框、Accessibility、Browser Tab 和 Clipboard 验收。

## 4. 测试准备

### 4.1 测试环境

- [ ] macOS 已开启 Microphone 权限
- [ ] macOS 已开启 Accessibility 权限
- [ ] Browser Automation 权限按测试项开启或关闭
- [ ] VoiceFlow 已配置有效 API Key
- [ ] 默认 activation mode 已确认
- [ ] 默认 delivery policy 为 auto
- [ ] 准备 TextEdit 或其他普通文本编辑器
- [ ] 准备 Cursor / VS Code
- [ ] 准备 Terminal / iTerm / Warp
- [ ] 准备 Mail / Outlook
- [ ] 准备 Slack / Teams
- [ ] 准备 Chrome / Safari
- [ ] 准备 Gmail、Google Docs、Google Search、GitHub
- [ ] 准备一个未知 App 和一个未知网站

### 4.2 Clipboard sentinel

测试开始前，将以下内容复制到 Clipboard：

    VOICEFLOW_CLIPBOARD_SENTINEL

- [ ] 记录测试前 Clipboard 内容
- [ ] selected-text capture 后验证是否恢复 sentinel
- [ ] Cmd+V 前取消后验证是否恢复 sentinel
- [ ] history-only 验证不会覆盖 sentinel

### 4.3 标准普通口述

    嗯我想告诉 Mike 会议改到周五下午三点，金额是 1250 美元，链接是 https://example.com/docs，路径是 /Users/mingjie/app，运行 npm run build

必须检查：

- [ ] filler 被清理
- [ ] Mike 保留
- [ ] 周五下午三点保留
- [ ] 1250 美元保留
- [ ] URL 保留
- [ ] path 保留
- [ ] command 保留
- [ ] 没有新增事实
- [ ] 没有自动添加标题、问候、签名或结论

### 4.4 标准明确指令

    帮我把这段话写得正式一点，我想告诉 Mike 会议改到周五下午三点

- [ ] operation 解析为 formalize
- [ ] 指令本身不进入最终输出
- [ ] Mike、日期和时间保持不变
- [ ] 输出比默认 cleanup 更正式
- [ ] 不新增 subject、greeting 或事实

## 5. Native App Context

### 5.1 Native App 识别矩阵

对每个 App：打开 App，点击真实可编辑输入框，等待 context preview 刷新，使用标准普通口述，然后检查 context policy、最终文本、目标输入框和 History。

| App | 预期 family | 通过 |
| --- | --- | --- |
| Cursor | PromptOrCode; field kind from focused AX cues | [ ] |
| VS Code | PromptOrCode; field kind from focused AX cues | [ ] |
| Terminal / iTerm / Warp | Terminal | [ ] |
| Apple Mail / Outlook | Email | [ ] |
| Slack / Microsoft Teams | WorkChat | [ ] |
| WeChat / Messages / Discord | PersonalChat | [ ] |
| Notion / Microsoft Word | Document | [ ] |
| Calendar / Reminders | CalendarTask | [ ] |

### 5.2 Native App 预期

- [ ] Cursor / VS Code 只有 focused `AXDescription` / `AXTitle` 明确标出 chat / composer / prompt 时才分类为 `CodingPrompt`
- [ ] Cursor / VS Code 的 editor 和 terminal 需各自有 focused field cues；缺少或通用 field metadata 保持 `Unknown`
- [ ] Cursor / VS Code 保留代码、路径、命令、版本和技术 token
- [ ] Cursor / VS Code 不自动生成未说出的代码
- [ ] Terminal 保留 flags、path、引号、大小写和 command syntax
- [ ] Terminal 不新增 shell command，也不把 command 改成解释文字
- [ ] Mail / Outlook 不自动添加 subject、greeting、签名或承诺
- [ ] Slack / Teams 使用简洁 WorkChat 风格
- [ ] WeChat / Messages / Discord 保留口语感
- [ ] Notion / Word 不自动总结、生成标题或 bullet list
- [ ] Calendar / Reminders 不新增未说出的日期、地点或参与者
- [ ] 切换到其他 App 后不会写入旧目标

## 6. Browser Domain Context

### 6.1 Domain 矩阵

| 网站 | 预期 family | 通过 |
| --- | --- | --- |
| Gmail / mail.google.com | Email | [ ] |
| Outlook | Email | [ ] |
| Slack / Teams | WorkChat | [ ] |
| Notion / Google Docs / Google Drive | Document | [ ] |
| Google / Bing / DuckDuckGo | BrowserSearch | [ ] |
| GitHub / GitLab | DeveloperCollaboration by domain; GitHub issue / PR fields use focused-field policy; positive Search uses BrowserSearch | [ ] |
| Linear / Asana / Trello | ProjectManagement | [ ] |
| Google Calendar / Todoist | CalendarTask | [ ] |
| X / Twitter / Reddit | SocialMedia | [ ] |
| 未知网站 | General | [ ] |

### 6.2 Domain policy

- [ ] Gmail / Outlook 不添加 subject、greeting 或签名
- [ ] Slack / Teams 使用简洁 WorkChat 风格
- [ ] Search 保留关键词结构，不扩写成长文章
- [ ] Google Docs / Notion 保留段落，不自动总结
- [ ] GitHub / GitLab 保留 issue、URL、path、command 和技术词
- [ ] GitHub issue / pull-request path 与 focused field label 同时明确时，title 是 Form、body / description 是 Document、reply / comment / review 是 Chat；缺任一证据就保持通用 field 分类
- [ ] Project Management 不新增负责人、日期或项目事实
- [ ] Calendar / Todoist 保留日期、时间、地点和 next action
- [ ] SocialMedia 保留原始语气和自然简洁度
- [ ] 未知网站进入 General Faithful Cleanup

### 6.3 Browser permission

- [ ] Browser Automation 权限允许时可以读取 active domain/tab
- [ ] Browser Automation 权限关闭时不会误判为安全目标
- [ ] 预期 host 存在但 current host 缺失时 fail-closed
- [ ] 预期 tab token 存在但 current token 缺失时 fail-closed
- [ ] 权限或目标不可确认时复制到 Clipboard
- [ ] History 保存正确 fallback reason
- [ ] 不显示普通 done

## 7. Window 和 Browser Tab Context

### 7.1 Window

- [ ] 同一个 App 打开两个 Window
- [ ] 两个 Window title 不同
- [ ] 两个 Window title 相同
- [ ] 录音开始后切换 Window
- [ ] ASR 期间切换 Window
- [ ] cleanup 期间切换 Window
- [ ] delivery 前切换 Window
- [ ] Undo 前切换 Window
- [ ] Window 变化时不写入旧 Window
- [ ] 无法确认 Window 时 Clipboard fallback
- [ ] Undo 目标变化时返回 stale_target
- [ ] 不发送盲目 Cmd+Z

### 7.2 Browser Tab

- [ ] 同一个 Browser 切换到不同 URL Tab
- [ ] 切换到相同 URL、不同 title 的 Tab
- [ ] 切换到相同 URL、不同 tab index 的 Tab
- [ ] ASR 期间切换 Tab
- [ ] cleanup 期间切换 Tab
- [ ] delivery 前切换 Tab
- [ ] Undo 前切换 Tab
- [ ] Tab 未变化时继续交付
- [ ] Tab 变化时 stale_target 或 Clipboard fallback
- [ ] 绝不写入错误 Tab
- [ ] Undo 不对新 Tab 发送 Cmd+Z
- [ ] 同 URL Tab 也能通过 token 区分

## 8. Focused Input Context

测试以下 input 类型：

- [ ] 普通单行文本框
- [ ] 多行 textarea
- [ ] Search input
- [ ] Code editor
- [ ] Terminal prompt
- [ ] Email compose
- [ ] Chat input
- [ ] Document editor
- [ ] Form field
- [ ] Password / secure field
- [ ] 没有 focused input
- [ ] focused input 在处理过程中消失

预期：

- [ ] 普通 editable input 可以尝试写入
- [ ] Secure field 禁止自动注入
- [ ] Unknown field 不自动注入
- [ ] 任何 positive Search field 都使用 BrowserSearch 风格；显式 AppMapping 仍优先
- [ ] GitHub issue / pull-request 的 title、body / description 和 comment / reply 按正向 AX field label 区分；GitHub host 或 page path 缺失时不应用该细分
- [ ] 输入框变化时 fail-closed
- [ ] 不写入错误控件
- [ ] AX 无法验证时显示 unverified 或 Clipboard fallback
- [ ] unverified 不提供 Undo

## 9. App Mapping Context

- [ ] 创建 Native bundle mapping
- [ ] 创建 executable mapping
- [ ] 创建 Browser host mapping
- [ ] mapping label 正确
- [ ] mapping family 正确
- [ ] mapping writing mode 正确
- [ ] enabled mapping 生效
- [ ] disabled mapping 不生效
- [ ] 删除 mapping 后不再生效
- [ ] 重启后 mapping 仍然存在
- [ ] 多个 mapping 冲突时结果稳定
- [ ] mapping 无效时回退到系统 context
- [ ] mapping 不改变真实 target guard

故意设置一个不同 mapping，例如 Gmail → PromptOrCode：

- [ ] Gmail cleanup policy 按 mapping 改变
- [ ] 结果仍然写入 Gmail 当前 input
- [ ] 不会因为 mapping 把文字注入 Cursor
- [ ] mapping 不改变 PID、Window、Browser Tab 或 input token

## 10. Writing Mode Context

内置 writing modes：

- [ ] Email
- [ ] Browser Search
- [ ] Work Chat
- [ ] Personal Chat
- [ ] Document
- [ ] Project Management
- [ ] Calendar / Task
- [ ] Developer Collaboration
- [ ] Prompt / Code
- [ ] Terminal
- [ ] Form Filling
- [ ] Notes / Journaling
- [ ] Social Media
- [ ] Customer Support
- [ ] General

每个 mode 都要验证：

- [ ] label 正确
- [ ] prompt 正确加载
- [ ] formality 正确
- [ ] density 正确
- [ ] markup policy 正确
- [ ] technical token policy 正确
- [ ] 不新增事实
- [ ] 不覆盖明确语音指令
- [ ] cleanup disabled 时不会调用 AI 改写

Custom writing mode：

- [ ] 创建 custom mode
- [ ] 修改 prompt 后保存并生效
- [ ] 重启后仍然存在
- [ ] 删除后不再生效
- [ ] 空 label 被拒绝
- [ ] 空 prompt 被拒绝
- [ ] 超过字符限制被拒绝
- [ ] 重复 ID 被拒绝
- [ ] custom mode 不改变 target identity

## 11. Manual Context Override

- [ ] Gmail 临时切换为 General
- [ ] Slack 临时切换为 Email
- [ ] Cursor 临时切换为 Document
- [ ] override 后执行普通 cleanup
- [ ] override 后执行 explicit rewrite
- [ ] 清除 override
- [ ] 重启后 override 不持久化
- [ ] override 只改变 policy，不改变 target guard
- [ ] override 不改变 PID / Window / Tab / input token

## 12. Context Confidence

当前规则：

    confidence >= 0.75 → 使用具体 App/context policy
    confidence < 0.75  → General Faithful Cleanup

- [ ] 高置信度 Cursor 使用 Code policy
- [ ] 高置信度 Gmail domain 使用 Email policy
- [ ] 高置信度 user mapping 使用 mapping policy
- [ ] 低置信度 Search field 不激进格式化
- [ ] 低置信度 Chat field 不自动变正式
- [ ] 低置信度 unknown browser 使用 General cleanup
- [ ] 完全未知 App 使用 General cleanup
- [ ] 低置信度不自动生成标题
- [ ] 低置信度不自动扩写内容
- [ ] 低置信度仍保留事实、原语言和混合语言

## 13. Context 与 Prompt 优先级

固定优先级：

    明确语音指令
    > 显式 output mode
    > confirmed App mapping
    > manual override
    > 高置信度 App context
    > General Faithful Cleanup

- [ ] Gmail context + “改得正式一点” → formalize
- [ ] Cursor context + “整理一下” → faithful cleanup
- [ ] Terminal context + “写得自然一点” → 不破坏 command syntax
- [ ] Slack context + “改成邮件” → explicit instruction 优先
- [ ] Manual Email override + “口语一点” → explicit instruction 优先
- [ ] Search context + 普通口述 → 保持 search query 结构
- [ ] Unknown App + 普通口述 → General cleanup
- [ ] transcript 中未经解析的指令不会被执行
- [ ] selected text 中的 prompt injection 不会被执行

## 14. Selected Text Context

### 14.1 选中文本捕获

- [ ] TextEdit 选择文字
- [ ] Cursor 选择代码
- [ ] Gmail 选择邮件内容
- [ ] Slack 选择聊天内容
- [ ] Browser 选择网页文字
- [ ] 启动 selected-text hotkey
- [ ] 正确读取选中文本
- [ ] 原 Clipboard sentinel 被恢复
- [ ] 选中文本不会自动写入普通 History
- [ ] 选中文本不会自动生成个人词典

### 14.2 Selected text + voice instruction

分别测试：

    把这段话改得正式一点
    帮我缩短
    改成更口语
    翻译成英文
    整理成三个 bullet points

- [ ] 选中文本作为处理对象
- [ ] 语音指令不进入最终输出
- [ ] preview 出现
- [ ] preview 不自动替换原文
- [ ] preview 可以编辑
- [ ] URL、path、command、name、number 等 protected facts 保留
- [ ] 选中文本内的 prompt injection 只作为数据处理，不会被执行

### 14.3 Preview confirm / replace

- [ ] 不修改 preview 直接确认
- [ ] 修改 preview 后确认
- [ ] preview 为空时拒绝
- [ ] preview 超长时拒绝
- [ ] 原 App 仍 active 时替换成功
- [ ] 原 Window 仍一致时替换成功
- [ ] 原 Browser Tab 仍一致时替换成功
- [ ] 原 selection 仍一致时替换成功
- [ ] verified paste 才显示 done
- [ ] unverified paste 不显示 Undo

### 14.4 Preview target changed

- [ ] preview 生成后切换 App
- [ ] preview 生成后切换 Window
- [ ] preview 生成后切换 Browser Tab
- [ ] preview 生成后修改原选区
- [ ] preview 生成后取消原选区
- [ ] 不替换错误目标
- [ ] 只复制结果到 Clipboard
- [ ] stale preview 不能继续执行
- [ ] 取消后不能再次 confirm
- [ ] 取消后不能再次 copy
- [ ] 不产生迟到 delivery

### 14.5 Copy-only / Cancel

- [ ] Copy-only 不发送 Cmd+V
- [ ] Copy-only 正确覆盖 Clipboard
- [ ] Copy-only 不修改目标 App
- [ ] Cancel 清除内存 preview
- [ ] Cancel 后 generation 增加
- [ ] 普通 dictation 可以在 preview cancel 后重新开始

## 15. Clipboard Safety Context

Clipboard 不是 LLM context，而是 delivery safety context。

- [ ] selected text capture 后恢复原 Clipboard
- [ ] Cmd+V 前取消后恢复原 Clipboard
- [ ] Cmd+V 后取消后保留结果 Clipboard
- [ ] Paste 失败后结果仍在 Clipboard
- [ ] Clipboard 读取失败时不继续覆盖用户 Clipboard
- [ ] History retry 使用 Clipboard-only
- [ ] history-only 不覆盖 Clipboard
- [ ] selected preview copy-only 正确覆盖 Clipboard
- [ ] 原 Clipboard 内容不会发送给 LLM

## 16. Delivery Policy 与 Context 组合

设置页面的 delivery policy：

- auto
- paste_shortcut
- clipboard_only
- history_only

至少运行以下组合：

| Context | Auto | Paste shortcut | Clipboard only | History only |
| --- | --- | --- | --- | --- |
| Cursor | [ ] | [ ] | [ ] | [ ] |
| Gmail | [ ] | [ ] | [ ] | [ ] |
| Slack | [ ] | [ ] | [ ] | [ ] |
| Google Search | [ ] | [ ] | [ ] | [ ] |
| Terminal | [ ] | [ ] | [ ] | [ ] |
| Unknown App | [ ] | [ ] | [ ] | [ ] |
| Selected Text | [ ] | [ ] | [ ] | [ ] |

每项必须检查：

- [ ] cleanup policy 正确
- [ ] target guard 正确
- [ ] final text 正确
- [ ] delivery method 正确
- [ ] History status 正确
- [ ] fallback reason 正确
- [ ] 没有写错 App / Window / Tab

## 17. Backend Undo Context Safety

当前 HUD 没有交付 Undo 按钮。以下条目核对保留的 `undo_last_delivery` 后端命令，使用隔离测试入口或后端测试；词典学习提示的「撤销」单独验证。

- [ ] verified paste 后 3 秒内调用 Undo 命令
- [ ] 超过 3 秒调用 Undo 命令
- [ ] 插入后继续编辑再调用 Undo 命令
- [ ] 插入后切换 App 再调用 Undo 命令
- [ ] 插入后切换 Window 再调用 Undo 命令
- [ ] 插入后切换 Browser Tab 再调用 Undo 命令
- [ ] 同窗口切换到文字、描述和几何位置均相同的另一输入框，Undo 返回 stale_target
- [ ] 粘贴读回期间快速 A → B → A，再切到 B 调用 Undo 命令；不能撤销 B
- [ ] 连续插入两次后调用 Undo 命令
- [ ] 连续两次调用 Undo 命令
- [ ] unverified paste 没有有效 Undo 事务

预期：

    目标未变化 + focused input 未变化 → 允许 Undo
    目标变化或用户继续编辑 → stale_target，不发送 Cmd+Z

## 18. Context Sources and Screen Images

### Current behavior and permissions

- `context_enabled` gates automatic content extraction. Local scene classification and minimum target / insertion safety metadata continue when it is off.
- Existing AppMappings now carry AND selectors for App bundle, executable, browser host, page path prefix, and focused field. Missing values fail any specified selector. Rank matching rules by host presence and specificity, path presence and specificity, focused field, App / executable selectors, selector count; lexicographically ascending mapping ID breaks only a complete specificity tie.
- `source_permissions.ax_text`, `local_ocr`, `cloud_vision`, and `context_text_to_providers` default to false. They are separate from scene metadata and from the global `window_ocr_enabled` OCR switch. The settings migration carries an explicitly enabled legacy global OCR choice only into `local_ocr` for mappings that existed at migration time; it grants neither AX text, provider text, nor cloud vision.
- AX, OCR, and vision evidence is bounded, source-tagged, session / target bound, in memory only, and never serialized to History or export. ASR receives relevant terms; cleanup receives only separately authorized snippets as untrusted data. OCR runs locally, but OCR-derived text may leave this Mac when `context_text_to_providers` is enabled.
- AX content sufficient for context skips image capture. If context remains insufficient, local OCR requires the per-App `local_ocr` grant, global OCR switch, and Screen Recording permission. Automatic cloud fallback additionally requires `cloud_vision`, `context_text_to_providers`, a concrete App / executable selector, configured vision capability, and Screen Recording permission. At most one current-window image is captured per recording and shared between OCR and cloud fallback; no whole-screen capture is used. Images stay in memory and never enter History / export / logs.
- Focused-field classification is app-aware only where positive local AX evidence exists. Known IDEs classify a field as `CodingPrompt` only when the focused description / title says chat, composer, or prompt; otherwise-unclassified or generic IDE fields stay `Unknown`. GitHub issue / pull-request refinement requires both `github.com` issue / PR path evidence and an editable-field label: title → `Form` / `FormFilling`, body / description → `Document`, and comment / reply / review → `Chat` within `DeveloperCollaboration`. The generic classifier remains the fallback for other sites and fields.
- A positively classified `Search` field selects the built-in `BrowserSearch` family before app / domain defaults, while preserving the built-in app identity. A matching explicit AppMapping still wins. Search classification requires focused-field evidence; no window-title or domain guess is used.
- Raw AX labels and browser paths are consumed only during local classification. They do not enter `ContextSnapshot`, History, HUD labels, or provider prompts; only the coarse field kind may be retained with a policy for safe retry behavior.
- Permission changes cancel pending prefetch, clear captured evidence, and advance an in-memory policy revision. Evidence from before a revoke stays unusable after a later regrant; stale captures and responses cannot update the current HUD. The original audio stays available to the normal context-free ASR / cleanup fallback. Provider, credential, and model selection remain the user's configured choices.
- Manual selected-text actions authorize only the selected text for that action. They do not attach automatic nearby AX content. The independent manual look-at-screen hotkey keeps its explicit per-action permission and preview; its grant does not enable automatic context.
- `style_examples_approved` is a separate default-off provider permission. Legacy style pairs remain in Settings but are omitted until reviewed and approved. Automatic three-hit short-style observations enter pending `style_drafts`; confirming a draft is explicit approval. Global and per-App learning switches and source permissions still apply. Secure fields and protected banking / HR / SSO / password-manager presets remain blocked.
- HUD source badge reports only the actually projected source (`none`, `ax`, `ocr`, or `cloud_vision`) with a fixed label and optional user-authored rule label. It never carries evidence text, URL, title, PID, or target identity. History stores no source evidence or image metadata.

### Automated regression coverage

Existing Rust fixtures cover selector AND semantics and deterministic specificity, host/path boundaries and missing evidence, browser query / fragment target changes, generic conservative IDE and chat/search field cases, text-off reader suppression, per-source projection gates, single-window capture sharing, sufficient-AX/OCR cloud fallback skipping, provider unavailable / timeout / cancellation / stale results, context grant revocation across an ASR retry with the same audio, safe evidence serialization, settings migration, and style approval / pending-draft rules. GitHub host/path refinement still requires native AX acceptance evidence. Fixture coverage is not native App or cloud-provider evidence.

### Native and provider E2E prerequisites / current evidence

Use synthetic blank documents and no personal workspace or inbox content. The Phase 3 handoff identifies TextEdit, Cursor, Chrome, Safari, and WeChat as installed targets for native checks. VS Code and Slack were absent in that handoff, and no vision model was configured there, so no native VS Code / Slack field evidence or cloud-vision success can be claimed from it. Static Cursor NLS / DOM hints are not AX observations. Final post-Phase 7 native AX / OCR / provider acceptance and full E2E remain pending; mark each native check only after observing it on the actual App and recording sanitized evidence outside the repository.

Native E2E checklist:

- [ ] With no content grant, ordinary dictation makes zero contextual AX-reader calls and captures no image.
- [ ] Enable only AX text for a synthetic TextEdit field; verify a matched rule and fixed `AX text` badge, with no raw title / text shown by HUD.
- [ ] In Cursor, verify native `AXTitle` / `AXDescription` / role cues for chat, editor, and terminal independently. Missing field metadata must stay unknown; do not treat arbitrary editor content as prompt instructions.
- [ ] In Chrome / Safari, verify host + path mapping and a query / fragment target switch against the same synthetic field; a stale response must be discarded.
- [ ] In WeChat, verify a synthetic composer and search field separately; do not read a real conversation.
- [ ] With synthetic blank content, test OCR-only local permission, provider-text off, Screen Recording denied / granted, and AX / OCR sufficient cases. Confirm the one-window capture counter or equivalent sanitized evidence.
- [ ] With a specifically configured vision model, verify one permitted image is shared with OCR then sent only as insufficiency fallback; revoke permission or cancel while waiting and confirm no late HUD / History change.
- [ ] Verify `style_examples_approved` off/on and learning disabled/enabled with synthetic drafts; no unapproved pair enters provider requests.
- [ ] Keep manual selected-text and manual look-at-screen tests separate from automatic context grants.

### 18.1 Image selection

Region selection / crop UI is not implemented. The supported image scope is one current window; there is no full-screen capture or selected-region path.

## 19. History 与 Context 结果

- [ ] 成功记录保存 raw text 和 final text
- [ ] History 保存正确 cleanup status
- [ ] History 保存正确 degraded reason
- [ ] History 保存正确 delivery method
- [ ] History 保存正确 fallback reason
- [ ] AI 失败后显示 degraded，而不是普通 done
- [ ] history-only 显示 history / Archive indicator
- [ ] unverified 显示 warning，不显示 Undo
- [ ] raw text 重新整理不会覆盖旧 revision
- [ ] revision diff 能看到 raw → final 的变化
- [ ] History re-clean 不污染 dictation HUD
- [ ] History retry 不会写入当前 App

## 20. 自动化验证记录

当前自动化门禁（不要抄旧数字，跑完再勾）：

- [ ] Rust：`cargo test --manifest-path src-tauri/Cargo.toml`
- [ ] Frontend：`npm test -- --run`
- [ ] Strict Clippy 通过
- [ ] TypeScript lint 通过
- [ ] Production build 通过
- [ ] Rust fmt 通过
- [ ] git diff --check 通过

推荐命令：

    cargo fmt --manifest-path src-tauri/Cargo.toml --all
    cargo test -q --manifest-path src-tauri/Cargo.toml
    cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --all-features -- -D warnings
    npm test -- --run
    npm run lint
    npm run build
    git diff --check

## 21. 最终通过标准

一个 context 测试只有同时满足以下条件才算通过：

- [ ] Context source 正确
- [ ] Context family 正确
- [ ] Confidence 行为正确
- [ ] Prompt policy 正确
- [ ] protected facts 保留
- [ ] target guard 正确
- [ ] delivery method 正确
- [ ] fallback 行为正确
- [ ] History 状态正确
- [ ] 没有泄露 raw URL、PID、Window title 或 Clipboard
- [ ] App / Window / Tab 切换不会误写
- [ ] Undo 不会撤销用户后续编辑

真实 macOS 外部 App、Browser Tab、Accessibility、输入框写入、Clipboard 和当前默认整理模型质量评测完成前，不能把产品标记为 prod-ready.

## 可选录音与启动验收

- [ ] 「编辑哪种语气」只选择编辑对象；试跑旁标明当前草稿或已保存配置。在「语气」输入脱敏样例：默认无请求；点击试跑才处理，未保存修改会对比 saved / draft，模型、本地-only、provider fallback 和 guard fallback 分别标记。关闭或修改 Prompt / 样例后，迟到结果不会出现，也没有 History / App 写入。
- [ ] 录音与输出里设置翻译快捷键和目标语言；全局输出保持原模式。按它启动时 HUD 显示目标语言，结束后普通热键恢复原模式。取消 / 启动失败不留下翻译模式；用另一枚键结束现有录音不切换模式，松开非启动键不结束按住录音。
- [ ] 翻译键与主键 / 选中操作 / 看屏幕 / 跳过整理键的别名冲突被拒绝，Backspace / Delete 清空、Esc 保留。主键变更或注册失败后所有辅助热键可继续使用；设置翻译键不修改全局按法。
- [ ] 当前场景整理关闭或缺少凭据时，翻译键在采集前报错；严格离线没有云请求。provider 失败和保护拒绝继续显示现有回退状态，不标成已翻译成功。
- [ ] 翻译键直接口述正文，无「翻译成……」前缀也按固定目标处理；轻量 / 标准 / 重度与长短录音共享 Translate 授权，正文提及其他语言不切换目标。AssemblyAI 用 raw Sync 后的共同整理翻译。

以下项目需在实际 macOS 应用包上手工执行；单元测试不能替代这些结论。

- [ ] 麦克风连续三次录音：同设备复用，Idle 不产生 spool、预取或上传；关闭选项和权限撤销释放设备。
- [ ] 合盖切换到指定输入设备；不存在的设备明确报错；开启／关闭盖后下一会话重新选择。
- [ ] 普通及跳过整理热键短按、按住、Idle release、取消和重新注册行为正确；AssemblyAI raw Sync 和共同 LLM 禁用可从本次请求验证。
- [ ] 250ms 尾部音频、取消即时性、自动上限、设备断开和 Soniox 完整恢复。
- [ ] 冷启动及第二实例 CLI 动作、登录 --background 不显示或抢焦点；手动打开显示设置；系统登录启动状态与 UI 一致。
- [ ] Secure Input + 已确认复制、剪贴板写入失败、输入状态不确定的文案分别正确。
- [ ] macOS 打包后存在反馈 WAV、唯一 release-notes 来源、MLX sidecar、Metal 资源和第三方声明；开始／结束提示音及更新说明可用。

## 首次使用、麦克风自检和学习反馈验收

- [ ] 新配置默认只显示云端 / 本机路线，展开高级项能选择服务商、模型和区域；已有非默认服务商仍保留并自动展开。
- [ ] On Device 就绪后引导关闭云端整理，Cohere 固定语言与运行时能力检查不能跳过；完成听写试用后可直接完成，也可主动试用选中文本。
- [ ] 只进入主流程时保留已有文字操作开关与快捷键，录音 / 处理期间不能用导航离开试用。
- [ ] 完成保存和读取设置期间不能进入可选试用；复制、仅历史、未确认和降级完成后，已打开的词典页刷新实际替换次数。
- [ ] 麦克风自检不触发服务请求或创建音频文件；实际设备、合盖选择、增益与显示一致；验证安静输入、较轻声音、过高峰值、断开设备、权限撤销。
- [ ] 自检最多 30 秒；停止 / 关闭设置 / 隐藏窗口或失去焦点 / 切换设备或增益时结束；正式听写优先且迟到自检取消不能停止录音；开启常开麦克风时恢复空闲流。
- [ ] 真实确认或自动晋升的词在后续本机替换时累计处理次数；正确原稿、提示词命中、保守场景与语气试跑不计入。History 重新整理按同一语义统计，不把次数当作交付或准确率。
- [ ] 学习反馈读取失败显示重试且保留最近成功数据；没有生效规则的词条隐藏收益；旧数据库迁移不回填收益，清空全部数据删除新表及迁移备份。

- [ ] 服务在当前配置与其他服务之间移动时，每个服务只有一个密钥输入，草稿及错误不丢失；打开、收起管理区不增加探测。
- [ ] 历史加载更多后同日标题不重复；顺序、查询和分页保持原样。更多操作支持方向键、Escape、外部点击与焦点返回，编辑时菜单不可用。
