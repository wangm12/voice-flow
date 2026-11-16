# VoiceFlow 视觉设计规范与 Liquid Glass 指南

本文记录 VoiceFlow 当前产品界面实际采用的视觉规则，供后续设置页、通用控件和 HUD 设计复用。它总结现有实现，不替代组件代码中的精确行为；如文档与实现不一致，应先核实源码并同步修正文档。

## 设计方向

VoiceFlow 的设置界面以 macOS 原生感、安静的中性色和清晰分组为基础。用文字层级、对齐、间距和轻量分隔表达信息结构；颜色主要负责主题、操作状态和风险语义。页面优先复用现有 Settings 组件与语义 token，不为相似页面另外造一套控件。

- 保持克制、紧凑、可扫描；把最重要的设置放在页面上方。
- 用一层有意义的表面承载一组相关设置，组内用分隔线，不重复套卡片。
- 常用操作、说明、异常和保存结果必须明确；状态不能只靠颜色传达。
- 玻璃、渐变和阴影只用于确实需要材质表现的区域，不扩散到普通设置控件。
- 中文和日文保持正常字距，避免为了英文视觉效果压缩 CJK 字形。

## 主题与颜色

设置主界面支持浅色和深色。系统默认色彩方案由系统偏好决定，用户保存的主题选择优先。颜色从语义 token 读取，组件不应复制一套独立的灰阶或状态色。

| 语义 | 深色基准 | 浅色基准 | 用途 |
| --- | --- | --- | --- |
| base | oklch(0.204627 0 0) | oklch(0.985104 0 0) | 页面主画布 |
| card | oklch(0.276848 0 0) | oklch(1 0 0) | 设置分组表面 |
| elevated | oklch(0.305191 0 0) | oklch(0.958147 0 0) | 输入框、次级控件表面 |
| primary text | oklch(0.943089 0 0) | oklch(0.243535 0 0) | 标题和主要内容 |
| secondary text | oklch(0.782604 0 0) | oklch(0.496156 0 0) | 描述和辅助信息 |
| border | oklch(0.348460 0 0) | #E7E7E7 | 卡片边界和行分隔 |
| soft accent | oklch(0.321093 0 0) | oklch(0.915831 0 0) | 选中、悬停和轻强调背景 |
| focus | oklch(0.873021 0 0) | oklch(0.489056 0 0) | 键盘焦点环 |
| semantic status | success / warning / error | 对应深浅主题 ink | 成功、提醒和错误状态 |

上表的主题基准来自 [App.css](../../src/App.css)；精确值、额外操作态和系统主题映射以该文件为准。主操作使用单色高对比表面；次级和幽灵操作使用 elevated、accent-soft 等中性表面。危险操作才使用错误色。不要把蓝紫渐变当作默认强调色。

## 字体和文字层级

- 使用系统字体栈：macOS 优先 SF Pro Text，其他平台回落到系统 UI 字体。
- 页面标题为 28 px / 34 px、Semibold；标题后的说明为 13 px / 20 px。
- 分组标题为 16 px / 24 px、Medium；设置项名称为 14 px / 20 px、Medium。
- 描述通常为 13 px / 20 px；紧凑标签和状态通常为 12 px。
- 中文、日文标题和正文的 letter spacing 设为 0；混排时检查标点、换行和系统回退字形。
- 关键错误原因、当前值和主要动作不应仅靠省略号隐藏；长内容要允许换行或提供可访问的完整文本。

页面标题和设置组件由 [SettingsLayout.tsx](../../src/components/SettingsLayout.tsx) 提供；全局字体与 CJK 字距规则位于 [App.css](../../src/App.css)。

## 页面框架和间距

| 项目 | 当前基准 |
| --- | --- |
| 设置侧栏宽度 | 14 rem |
| 内容最大宽度 | 768 px，不含页面两侧留白 |
| 桌面页面留白 | 32 px |
| 视口宽度不超过 820 px 时页面留白 | 24 px |
| 设置行垂直留白 | 16 px |
| 设置行水平内边距 | 20 px |
| 紧凑控件和组内常用间距 | 8 px |
| 相邻设置分组间距 | 24 px |
| 分组标题到底层表面 | 约 8 px |

这些尺寸以 [App.css](../../src/App.css) 与 [SettingsLayout.tsx](../../src/components/SettingsLayout.tsx) 为准。内容区和导航区在各自滚动容器中滚动；macOS 窗口标题栏留白由平台布局处理。窄容器下，表单控制换到标签下方并占满可用宽度，避免横向挤压或溢出。

需要底部保存操作的页面沿用现有 save bar / engine footer 结构：操作栏与内容页边距对齐，使用一条顶部细分隔和 card 背景；滚动内容要能完整到达操作栏上方，操作栏不能盖住最后一行。窄容器下按钮改为可触达的整行布局。

## 表面、边角和分隔

- **SettingsGroup** 是设置页的标准分组：一个 card 表面、细边界、约 16 px 圆角，行之间用主题 border 分隔。
- **SettingsRow** 在同一分组内保持一致的水平和垂直 padding；每行的标签、描述和控件通过两列布局对齐。
- **SettingsDisclosure** 只在确有渐进展开需求时使用。普通辅助内容使用 plain disclosure；需要独立承载内容时才用带表面的 disclosure。
- 避免“卡片里面再套卡片”。优先把相关行合并到一个分组表面，以间距、细分隔线和标题表达层级。
- 通用控件使用 12 px 左右圆角。完整圆形只用于有语义的胶囊、状态点或小标签，不用于普通按钮。
- 阴影集中用于浮层和需要离开页面层级的元素；普通设置卡片不额外叠强阴影。

## 控件和交互状态

- 复用 [Select.tsx](../../src/components/Select.tsx)、[Toggle.tsx](../../src/components/Toggle.tsx) 和 [theme.ts](../../src/lib/theme.ts) 中的共用控件与按钮样式。
- 常规按钮、输入框和 Select 高度为 40 px，圆角 12 px；紧凑按钮为 32 px。按钮主次由填充、文字色和语义决定，不靠随意改变边角形状。
- 输入与 Select 使用 elevated 背景；悬停仅做轻量背景变化，禁用态用语义禁用颜色，不降低整行透明度。
- 键盘操作必须保留清楚的 2 px focus ring。错误、有效和禁用状态不能覆盖键盘焦点提示。
- 成功、警告、错误状态使用语义颜色并带文字；避免只用红绿颜色或小圆点表达状态。
- 普通文本与相邻背景的对比度以至少 4.5:1 为目标；大号文字和识别控件状态所需的非文字图形以至少 3:1 为目标。
- 保存、应用和异步反馈遵循该设置页已有的自动保存或显式保存契约，不用视觉样式暗示未实现的行为。

## 导航与滚动条

侧栏按产品任务分组，当前页面使用柔和的 accent 背景，其他项保持低对比；图标来自项目现有图标库并保持统一线宽。侧栏与主内容区各自滚动，但共用同一套 settings-scrollbar 样式，维持贴近各自容器右边缘、稳定 gutter 和一致的滑块宽度。滚动时临时显示滑块，停止后约 700 ms 隐藏；悬停或拖动时增强对比。实现见 [App.css](../../src/App.css)、[App.tsx](../../src/App.tsx) 和 [useSettingsScrollbar.ts](../../src/hooks/useSettingsScrollbar.ts)。

## 动效和响应式

- 动效服务于状态变化，不让选择控件、页面切换或 HUD 外观切换产生回弹、跳位或长时间等待。
- 普通状态淡入约 160 ms；交互过渡通常为 150 ms，并使用共享的 emphasized easing。
- 尊重系统 prefers-reduced-motion，减少或关闭非必要的动画和过渡。
- HUD 预览的自动状态轮播只用于展示，不写设置、不启动麦克风、不访问剪贴板；页面隐藏时暂停计时。
- 响应式布局通过容器宽度调整设置行和控件排列，保证长中文说明、翻译文本和操作按钮可读且可操作。

## HUD 胶囊基线

HUD 的主胶囊采用 220 × 48 pt 尺寸和 24 pt 圆角，按钮与状态布局由生产 VoicePill 共用。窗口宿主为 244 × 64 pt，以容纳内部安全区。此处应复用 [voicePillTokens.ts](../../src/components/Island/voicePillTokens.ts) 和生产组件；状态图集与尺寸演进记录在 [Voice HUD 设计参考](voice-hud/README.md)。

## Liquid Glass 设计指南

### 外观目标

Liquid Glass 应能看出背景透过单一玻璃表面产生的折射、轻微模糊和色彩变化。保持高光细、边缘克制、底色中性。深色使用烟灰石墨基调，避免整块偏蓝；浅色要保留可辨认的胶囊边缘和文字。玻璃不能靠白色实线描边来“画”出来。

### 实际 HUD 的材质与边界

- 实际 HUD 的玻璃由 macOS AppKit NSGlassEffectView 在原有 HUD WebView 后方绘制。WebView 仍负责文字、按钮和鼠标交互；玻璃材质仅提供底层表面。
- 原生玻璃使用 220 × 48 pt、24 pt 圆角，保持胶囊与生产 VoicePill 对齐。整个胶囊只出现一个玻璃边缘。
- WebView 文字与操作按钮使用对应前景和轻遮罩维持可读性；Liquid Glass 规则把 CSS 胶囊边框设为透明，不叠第二道描边或第二层玻璃。主胶囊在浅色下用 92% 不透明的白色 tint，深色下用 92% 不透明、基色为 `#191919` 的石墨 tint；原生材质显示和隐藏时沿用同一个 tint，避免收起过程换成更亮的实心壳；其余 8% 保留原生折射或背景内容的视觉贡献。
- 原生材质跟随已有的 HUD 操作、详情和恢复提示 owner。粘贴前的焦点交接只改变 key / mouse 归属，不提前撤掉仍可见胶囊的玻璃材质；确认粘贴后的收起规则不变。
- macOS 26 之前、系统开启“降低透明度”或原生安装失败时，实际 HUD 回退到 Classic。用户偏好仍可保留，但界面不得把未生效的偏好显示成已生效。
- 实际材质应只在 HUD 可见且原生确认已安装时显示；暖面板空闲、隐藏状态和终态收起时不显示玻璃层。

### 设置页预览

设置页预览用于观察胶囊在明暗样例背景和状态中的视觉效果，不等同于实际 AppKit 材质：

- 背景由本地合成样例生成；预览不读取桌面、活动窗口或窗口像素，也不请求屏幕录制权限。
- 折射预览使用固定上游版本的 Glasswork 单表面 WebGL2 shader；来源和 MIT 许可见 [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md)，适配代码见 [GlassworkRenderer.ts](../../src/components/settings/hud-glasswork/GlassworkRenderer.ts)。
- WebGL2 不可用、上下文丢失或系统降低透明度时使用 CSS 材质回退；降低透明度时改用不透明表面，不能依赖模糊来区分控件。
- 预览复用生产 VoicePill 的几何和内容布局，但浏览器合成与 NSGlassEffectView 不同，因此不应承诺像素级一致。预览应明确标为模拟效果。
- 深色预览采用中性石墨背景和轻灰玻璃色；浅色和深色都要检查文字、状态图标、停止按钮、胶囊边缘和背景细节。

### 实作检查清单

1. 确认实际 HUD 与设置预览只有一层材质边缘，CSS 胶囊没有额外白色描边或重复阴影。
2. 在浅色、深色和不同明暗样例背景上检查折射可见度与前景可读性。
3. 检查录音中、处理中和隐藏状态；隐藏时不要残留玻璃底板。
4. 验证 Classic 偏好、系统降低透明度、旧版 macOS、WebGL2 不可用和 WebGL 上下文丢失时的回退外观。
5. 用模拟数据预览时，不声称已经验证原生窗口效果；需要原生结论时在真实 AppKit HUD 单独验收。
6. 修改 Glasswork 派生代码或来源版本时，同步检查第三方许可和 source pin。

## 规范来源和更新规则

| 主题 | 当前实现来源 |
| --- | --- |
| 主题 token、设置布局、滚动条 | [App.css](../../src/App.css) |
| 页面标题、分组、设置行、状态提示 | [SettingsLayout.tsx](../../src/components/SettingsLayout.tsx) |
| Select、按钮、焦点环 | [Select.tsx](../../src/components/Select.tsx)、[theme.ts](../../src/lib/theme.ts) |
| HUD 胶囊尺寸和组件 | [voicePillTokens.ts](../../src/components/Island/voicePillTokens.ts)、[VoicePill.tsx](../../src/components/Island/VoicePill.tsx) |
| HUD 和 Glass 预览 | [HudAppearanceSettings.css](../../src/components/settings/HudAppearanceSettings.css)、[HudGlassPreview.tsx](../../src/components/settings/HudGlassPreview.tsx)、[island.css](../../src/island.css) |
| 原生玻璃材质和能力回退 | [native_glass.rs](../../src-tauri/src/native_glass.rs) |
| 当前 HUD 行为与隐私说明 | [end-to-end-workflows.md](../end-to-end-workflows.md)、[privacy.md](../privacy.md) |

更新全局风格时先复用或扩展现有语义 token 和共用组件，再补充本指南。局部例外应由明确的交互或平台需求支撑，并限制在对应组件；不要把一次性的局部数值提升为全局规则。行为变化同步维护活文档，HUD 状态图集仅作为视觉和状态参考。
