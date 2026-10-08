export type Locale = "zh" | "en";
export type Scene = "chat" | "email" | "code";
export type Phase =
  "idle" | "listening" | "processing" | "writing" | "complete";

export const links = {
  download:
    "https://github.com/wangm12/voice-flow/releases/latest/download/VoiceFlow.dmg",
  github: "https://github.com/wangm12/voice-flow",
  guide: "https://github.com/wangm12/voice-flow#readme",
  privacy: "https://github.com/wangm12/voice-flow/blob/main/docs/privacy.md",
  releases: "https://github.com/wangm12/voice-flow/releases",
} as const;

interface DemoScene {
  label: string;
  app: string;
  recipient: string;
  context: string;
  spoken: string;
  result: string;
}

interface Content {
  meta: { title: string; description: string };
  nav: {
    features: string;
    how: string;
    privacy: string;
    download: string;
    open: string;
    close: string;
    language: string;
    home: string;
    skip: string;
    label: string;
  };
  hero: {
    eyebrow: string;
    title: [string, string];
    description: string;
    download: string;
    demo: string;
    compatibility: string;
  };
  demo: {
    label: string;
    play: string;
    replay: string;
    pause: string;
    resume: string;
    paused: string;
    restarted: string;
    spoken: string;
    written: string;
    preview: string;
    placeholder: string;
    caption: string;
    appWindow: string;
    phases: Record<Phase, string>;
    scenes: Record<Scene, DemoScene>;
  };
  apps: { heading: string; note: string; mail: string; wechat: string };
  how: {
    eyebrow: string;
    title: [string, string];
    description: string;
    shortcut: string;
    shortcutLabel: string;
    steps: { title: string; description: string }[];
  };
  features: {
    eyebrow: string;
    title: [string, string];
    description: string;
    cleanup: {
      eyebrow: string;
      title: [string, string];
      description: string;
      note: string;
      before: string;
      after: string;
      original: string;
      polished: string;
      badge: string;
    };
    dictionary: {
      eyebrow: string;
      title: [string, string];
      description: string;
      note: string;
      label: string;
      add: string;
      terms: string[];
      footer: string;
    };
    snippets: {
      eyebrow: string;
      title: [string, string];
      description: string;
      note: string;
      phrase: string;
      expansion: string;
      label: string;
      expanded: string;
    };
  };
  privacy: {
    eyebrow: string;
    title: [string, string];
    description: string;
    link: string;
    points: { title: string; description: string }[];
  };
  faq: {
    eyebrow: string;
    title: string;
    description: string;
    items: { question: string; answer: string }[];
  };
  closing: {
    eyebrow: string;
    title: string;
    description: string;
    github: string;
  };
  footer: {
    description: string;
    product: string;
    resources: string;
    guide: string;
    releases: string;
    made: string;
    copyright: string;
  };
}

export const content: Record<Locale, Content> = {
  zh: {
    meta: {
      title: "VoiceFlow — 少打字，多表达。",
      description:
        "少打字，多表达。VoiceFlow 是为 macOS 打造的语音输入工具，把你的声音变成清晰的文字，送回正在使用的应用。",
    },
    nav: {
      features: "功能",
      how: "使用方式",
      privacy: "隐私",
      download: "下载",
      open: "打开导航",
      close: "关闭导航",
      language: "Switch to English",
      home: "VoiceFlow 首页",
      skip: "跳到主要内容",
      label: "主导航",
    },
    hero: {
      eyebrow: "好想法，值得脱口而出",
      title: ["少打字，", "多表达。"],
      description:
        "自然地说，清晰地写。\n把声音变成文字，送回你正在使用的应用。",
      download: "下载 macOS 版",
      demo: "看看它如何工作",
      compatibility: "为 Mac 打造 · macOS 13 及以上",
    },
    demo: {
      label: "互动示例",
      play: "播放演示",
      replay: "再看一次",
      pause: "暂停演示",
      resume: "继续演示",
      paused: "演示已暂停",
      restarted: "演示已重新开始",
      spoken: "你说的",
      written: "写下的",
      preview: "效果预览",
      placeholder: "你的想法，会出现在这里。",
      caption: "预设动画示例 · 自动循环，可随时暂停 · 仅在浏览器本地播放",
      appWindow: "应用效果预览",
      phases: {
        idle: "一句话，让想法开始流动。",
        listening: "说出你的想法…",
        processing: "正在整理表达…",
        writing: "把想法，写成清楚的文字。",
        complete: "成文，回到你的工作中。",
      },
      scenes: {
        chat: {
          label: "回消息",
          app: "团队聊天",
          recipient: "设计小组",
          context: "项目讨论",
          spoken:
            "嗯，跟大家说一下，设计稿已经更新了。然后，周四……不对，周五之前给我反馈吧，谢谢。",
          result: "设计稿已更新，请大家在周五前给我反馈。谢谢！",
        },
        email: {
          label: "写邮件",
          app: "邮件",
          recipient: "发给：项目团队",
          context: "下周的项目评审",
          spoken:
            "帮我写封邮件，大家好，我们下周二下午两点开评审会。请提前看一下新设计稿，有问题就在文档里留言，谢谢大家。",
          result:
            "大家好，\n\n项目评审会定于下周二下午两点。请提前查看新设计稿，并将问题留在文档中。\n\n谢谢大家！",
        },
        code: {
          label: "写提示词",
          app: "编辑器",
          recipient: "开发助手",
          context: "项目上下文",
          spoken:
            "帮我用 TypeScript 写一个搜索组件，要支持键盘操作，输入的时候做防抖，哦，还有加载状态和空结果的提示。",
          result:
            "用 TypeScript 实现一个搜索组件，要求：\n\n• 支持键盘操作\n• 对输入进行防抖\n• 展示加载状态\n• 提供空结果提示",
        },
      },
    },
    apps: {
      heading: "你熟悉的应用，换一种输入方式。",
      note: "编辑器、文档、邮件、聊天。跟随你的光标，不打断你的思路。",
      mail: "邮件",
      wechat: "微信",
    },
    how: {
      eyebrow: "简单到，几乎不用想",
      title: ["从想法，到文字。", "只差开口。"],
      description: "保持在当前窗口，让输入成为工作中最自然的一步。",
      shortcut: "默认快捷键 · 可在设置中修改",
      shortcutLabel: "Command、Option 与空格键",
      steps: [
        {
          title: "按下快捷键",
          description: "把光标放在想输入的位置，用全局热键开始录音。",
        },
        {
          title: "像平常一样说话",
          description: "说出一句想法、一封邮件，或下一条开发提示词。",
        },
        {
          title: "文字回到光标处",
          description: "结束录音，转写结果经过可选整理，送回当前输入框。",
        },
      ],
    },
    features: {
      eyebrow: "为表达，少添一点阻力",
      title: ["随口说。", "也能好好写。"],
      description: "从零散想法到清楚的文字，保留你的意思，照顾表达的细节。",
      cleanup: {
        eyebrow: "可选智能整理",
        title: ["思路可以随意。", "表达可以清楚。"],
        description:
          "去掉重复和口头语，补上标点与结构，保留原本的意思。",
        note: "你决定是否启用 AI 整理，也可以为不同应用设置写作语气。",
        before: "随口说的",
        after: "整理后的",
        original:
          "嗯，今天有三件事，先更新文档，然后修一下那个登录问题，最后……对，把版本发布了。",
        polished: "今天的三件事：\n1. 更新文档\n2. 修复登录问题\n3. 发布新版本",
        badge: "保留意思，理顺表达",
      },
      dictionary: {
        eyebrow: "个人词典",
        title: ["专业词，名字。", "都是你的日常。"],
        description:
          "把项目名、技术术语和常用名字加入词典，为识别提供你自己的词汇。",
        note: "支持手动添加与文件导入。改正学习产生的候选，也可以由你复核。",
        label: "我的词典",
        add: "个人词汇示例",
        terms: ["VoiceFlow", "TypeScript", "王小明"],
        footer: "你的词汇，由你管理。",
      },
      snippets: {
        eyebrow: "语音片段",
        title: ["常用的一大段。", "现在，只用一句话。"],
        description:
          "把签名、常用回复或一段介绍存为语音片段。说出对应短语，就能展开完整内容。",
        note: "片段保存在本机，不会把展开内容发给语音服务。",
        phrase: "“我的签名”",
        expansion: "王小明\n产品设计师\n用细节，让工作更简单。",
        label: "你说",
        expanded: "展开为",
      },
    },
    privacy: {
      eyebrow: "隐私，是产品的一部分",
      title: ["你的声音。", "你的选择。"],
      description: "使用哪家服务、保留什么内容、授予哪些权限，都由你决定。",
      link: "了解数据与隐私",
      points: [
        {
          title: "密钥交给 Keychain",
          description: "新配置的 API 密钥保存于 macOS Keychain。",
        },
        {
          title: "历史留在本机",
          description: "听写记录在本地保存，可按需导出和删除。",
        },
        {
          title: "服务由你选择",
          description: "云端音频和文字发往你配置的服务，适用其数据政策。",
        },
        {
          title: "默认听写不截屏",
          description: "看屏幕是单独的热键操作，需要视觉模型与屏幕录制权限。",
        },
      ],
    },
    faq: {
      eyebrow: "开始之前",
      title: "你可能想知道。",
      description: "几件小事，先说清楚。",
      items: [
        {
          question: "VoiceFlow 可以在哪些应用里使用？",
          answer:
            "VoiceFlow 是 macOS 系统级语音输入工具，可用于编辑器、浏览器、邮件、聊天和文档等文字输入场景。自动输入需要辅助功能权限；目标无法确认时，会保留文字并提供可恢复的交付方式。",
        },
        {
          question: "需要自己配置模型服务吗？",
          answer:
            "云端转写和 AI 整理使用你选择的服务，通常需要配置对应的 API 密钥，两者可以来自不同服务商。服务费用与数据政策由所选服务商决定。设置页提供可用路线与配置说明。",
        },
        {
          question: "支持中文和英文吗？",
          answer:
            "设置中可以选择自动识别、中文或 English。实际语言与混合语言支持取决于所选转写服务和模型；个人词典可用于提供自己的专业术语。",
        },
        {
          question: "可以在本机处理语音吗？",
          answer:
            "当前项目提供可选 On Device 路线，需要 Apple Silicon、macOS 14 或更新版本，以及主动下载模型。所下载版本的可用功能请以发行说明为准；云端路线不会自动变成本地处理。",
        },
        {
          question: "这个网页会录音吗？",
          answer:
            "不会。网页演示使用预设的中英文示例，只在浏览器本地播放视觉过程，不打开麦克风、不上传音频，也不调用转写服务。实际语音输入请下载 macOS 应用。",
        },
      ],
    },
    closing: {
      eyebrow: "从下一句好想法开始",
      title: "让表达，自然发生。",
      description: "少一次敲击，多一点心流。",
      github: "在 GitHub 上了解更多",
    },
    footer: {
      description: "把声音，变成你的文字。",
      product: "产品",
      resources: "资源",
      guide: "使用说明",
      releases: "发行说明",
      made: "为 Mac 与你的好想法打造。",
      copyright: "© 2026 VoiceFlow",
    },
  },
  en: {
    meta: {
      title: "VoiceFlow — Less typing. More flow.",
      description:
        "Turn your voice into clear writing, right where you work. VoiceFlow is a macOS dictation tool for developers and people with ideas to put into words.",
    },
    nav: {
      features: "Features",
      how: "How it works",
      privacy: "Privacy",
      download: "Download",
      open: "Open navigation",
      close: "Close navigation",
      language: "切换到中文",
      home: "VoiceFlow home",
      skip: "Skip to main content",
      label: "Main navigation",
    },
    hero: {
      eyebrow: "GOOD IDEAS DESERVE TO BE SPOKEN",
      title: ["Less typing.", "More flow."],
      description:
        "Speak naturally. Write clearly.\nYour voice becomes text, right where you work.",
      download: "Download for macOS",
      demo: "See how it works",
      compatibility: "Made for Mac · macOS 13 and later",
    },
    demo: {
      label: "INTERACTIVE EXAMPLE",
      play: "Play demo",
      replay: "Play again",
      pause: "Pause demo",
      resume: "Resume demo",
      paused: "Demo paused",
      restarted: "Demo restarted",
      spoken: "WHAT YOU SAY",
      written: "WHAT YOU WRITE",
      preview: "Example preview",
      placeholder: "Your next thought goes here.",
      caption:
        "Preset animation · Loops automatically, pause anytime · Runs locally",
      appWindow: "Example app window",
      phases: {
        idle: "One thought. A little more flow.",
        listening: "Speaking your thoughts…",
        processing: "Tidying up the words…",
        writing: "Turning thoughts into clear writing.",
        complete: "Written. Back to your work.",
      },
      scenes: {
        chat: {
          label: "Messages",
          app: "Team chat",
          recipient: "Design team",
          context: "Project discussion",
          spoken:
            "Um, tell everyone the designs are updated. And could they send feedback by Thursday… actually, Friday? Thanks.",
          result:
            "The designs are updated. Please send your feedback by Friday. Thanks!",
        },
        email: {
          label: "Emails",
          app: "Mail",
          recipient: "To: Project team",
          context: "Next week’s project review",
          spoken:
            "Write an email. Hi team, we’ll have the review next Tuesday at two in the afternoon. Please look at the new designs first and leave any questions in the document. Thanks everyone.",
          result:
            "Hi team,\n\nOur project review is next Tuesday at 2 pm. Please review the new designs beforehand and leave your questions in the document.\n\nThanks!",
        },
        code: {
          label: "Prompts",
          app: "Editor",
          recipient: "Coding assistant",
          context: "Project context",
          spoken:
            "Build a search component in TypeScript. It needs keyboard support, debounce the input, and, oh, include a loading state and an empty results message.",
          result:
            "Build a search component in TypeScript with:\n\n• Keyboard support\n• Debounced input\n• A loading state\n• An empty results message",
        },
      },
    },
    apps: {
      heading: "Your favorite apps. A new way to write.",
      note: "Editors, documents, email, and chat. Follow your cursor. Stay with your thoughts.",
      mail: "Mail",
      wechat: "WeChat",
    },
    how: {
      eyebrow: "SECOND NATURE, FROM THE FIRST WORD",
      title: ["From a thought", "to a sentence."],
      description:
        "Stay in your window. Let writing become the most natural part of your work.",
      shortcut: "Default shortcut · customizable in settings",
      shortcutLabel: "Command Option Space",
      steps: [
        {
          title: "Press your shortcut",
          description:
            "Put your cursor where you want to write. Start recording with your global hotkey.",
        },
        {
          title: "Speak your mind",
          description:
            "A quick thought, an email, or your next coding prompt. Say it in your own words.",
        },
        {
          title: "Keep your flow",
          description:
            "Finish recording. Your transcription, with optional cleanup, returns to the text field.",
        },
      ],
    },
    features: {
      eyebrow: "A LITTLE LESS FRICTION",
      title: ["Casually spoken.", "Clearly written."],
      description:
        "Turn scattered thoughts into readable words, with your meaning intact.",
      cleanup: {
        eyebrow: "OPTIONAL AI CLEANUP",
        title: ["Let thoughts wander.", "Let words make sense."],
        description:
          "Remove filler words and repetition. Add punctuation and structure, while keeping your meaning.",
        note: "Choose when to use AI cleanup, and set a writing tone for different apps.",
        before: "WHAT YOU SAY",
        after: "TIDIED UP",
        original:
          "Um, three things today. Update the docs, then fix that login thing, and finally… right, ship the release.",
        polished:
          "Three things for today:\n1. Update the documentation\n2. Fix the login issue\n3. Ship the release",
        badge: "Your meaning, a little clearer",
      },
      dictionary: {
        eyebrow: "PERSONAL DICTIONARY",
        title: ["Your terms. Your names.", "Your everyday words."],
        description:
          "Add project names, technical terms, and familiar names to give transcription your own vocabulary.",
        note: "Add words manually or import a file. Review suggestions from correction learning before accepting them.",
        label: "My dictionary",
        add: "Example personal vocabulary",
        terms: ["VoiceFlow", "TypeScript", "Maya Chen"],
        footer: "Your vocabulary. Managed by you.",
      },
      snippets: {
        eyebrow: "VOICE SNIPPETS",
        title: ["A whole paragraph.", "Just a few words."],
        description:
          "Save a signature, a frequent reply, or an introduction as a voice snippet. Speak the phrase to expand the full text.",
        note: "Snippets stay on your Mac. Expanded content is not sent to speech services.",
        phrase: "“My signature”",
        expansion:
          "Maya Chen\nProduct designer\nMaking work simpler, one detail at a time.",
        label: "YOU SAY",
        expanded: "EXPANDS TO",
      },
    },
    privacy: {
      eyebrow: "PRIVACY IS PART OF THE PRODUCT",
      title: ["Your voice.", "Your choices."],
      description:
        "You choose the services, what to keep, and which permissions to grant.",
      link: "Read about data & privacy",
      points: [
        {
          title: "Keys in Keychain",
          description: "New API credentials are stored in macOS Keychain.",
        },
        {
          title: "History stays local",
          description:
            "Dictation history is stored on your Mac. Export or delete it when you need to.",
        },
        {
          title: "Choose your services",
          description:
            "Cloud audio and text go to the services you configure, under their data policies.",
        },
        {
          title: "No default screenshots",
          description:
            "Look at screen is a separate hotkey, requiring a vision model and Screen Recording permission.",
        },
      ],
    },
    faq: {
      eyebrow: "BEFORE YOUR FIRST WORD",
      title: "A few good questions.",
      description: "A little clarity before you begin.",
      items: [
        {
          question: "Which apps can I use VoiceFlow in?",
          answer:
            "VoiceFlow is a system-level macOS dictation tool for editors, browsers, email, chat, and documents. Automatic insertion requires Accessibility permission. If the target cannot be verified, your text is kept recoverable through a safe delivery fallback.",
        },
        {
          question: "Do I need to configure a model service?",
          answer:
            "Cloud transcription and AI cleanup use the services you choose, usually with your own API keys. They can use different providers. Fees and data policies depend on those providers. The app’s settings explain the available routes and configuration.",
        },
        {
          question: "Does it support Chinese and English?",
          answer:
            "Settings offer automatic detection, Chinese, or English. Language and mixed-language support depend on your transcription provider and model. Your personal dictionary supplies terms that matter to you.",
        },
        {
          question: "Can I process speech on my Mac?",
          answer:
            "The current project offers an optional On Device route requiring Apple Silicon, macOS 14 or later, and an explicit model download. Check the release notes for features in your downloaded version. Cloud routes do not automatically become local processing.",
        },
        {
          question: "Does this website record audio?",
          answer:
            "No. The demo uses preset examples and plays a visual sequence locally in your browser. It does not open the microphone, upload audio, or call a transcription service. Download the macOS app for real dictation.",
        },
      ],
    },
    closing: {
      eyebrow: "START WITH YOUR NEXT GOOD IDEA",
      title: "Find your flow.",
      description: "A little less typing. A little more you.",
      github: "Explore on GitHub",
    },
    footer: {
      description: "Your voice. Your words.",
      product: "Product",
      resources: "Resources",
      guide: "User guide",
      releases: "Release notes",
      made: "Made for Mac. And your next good idea.",
      copyright: "© 2026 VoiceFlow",
    },
  },
};
