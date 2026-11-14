//! English is the default. Chinese covers the settings form and the sync page.

use crate::config::Language;

pub struct Copy {
    pub settings: &'static str,
    pub settings_dirty: &'static str,
    pub section_language: &'static str,
    pub section_network: &'static str,
    pub section_selection: &'static str,
    pub section_launch: &'static str,
    pub language: &'static str,
    pub language_en: &'static str,
    pub language_zh: &'static str,
    pub proxy_url: &'static str,
    pub proxy_no_proxy: &'static str,
    pub cache_ttl: &'static str,
    pub max_concurrent: &'static str,
    pub tui_refresh: &'static str,
    pub safety_margin: &'static str,
    pub team_priority: &'static str,
    pub restart_app_server: &'static str,
    pub restore_delay: &'static str,
    pub hint_language: &'static str,
    pub hint_proxy_url: &'static str,
    pub hint_proxy_no_proxy: &'static str,
    pub hint_cache_ttl: &'static str,
    pub hint_max_concurrent: &'static str,
    pub hint_tui_refresh: &'static str,
    pub hint_safety_margin: &'static str,
    pub hint_team_priority: &'static str,
    pub hint_restart_app_server: &'static str,
    pub hint_restore_delay: &'static str,
    pub empty: &'static str,
    pub on: &'static str,
    pub off: &'static str,
    pub settings_help: &'static str,
    pub saved: &'static str,
    pub sync_tab: &'static str,
    pub sync_title: &'static str,
    pub sync_intro_1: &'static str,
    pub sync_intro_2: &'static str,
    pub sync_group: &'static str,
    pub sync_url: &'static str,
    pub sync_password: &'static str,
    pub sync_unset: &'static str,
    pub sync_set: &'static str,
    pub sync_pull: &'static str,
    pub sync_push: &'static str,
    pub sync_help: &'static str,
    pub picker_upload_title: &'static str,
    pub picker_download_title: &'static str,
    pub picker_upload_help: &'static str,
    pub picker_download_help: &'static str,
    pub sync_busy: &'static str,
    pub sync_saved: &'static str,
    pub all_accounts: &'static str,
    pub footer_choose: &'static str,
    pub footer_edit: &'static str,
    pub footer_pull: &'static str,
    pub footer_upload: &'static str,
    pub tab_accounts: &'static str,
    pub tab_providers: &'static str,
    pub tab_settings: &'static str,
    pub tab_logs: &'static str,
    pub tab_hint: &'static str,
    pub col_alias: &'static str,
    pub col_email: &'static str,
    pub col_plan: &'static str,
    pub col_status: &'static str,
    pub col_cards: &'static str,
    pub col_expires: &'static str,
    pub col_credits: &'static str,
    pub col_models: &'static str,
    pub col_base_url: &'static str,
    pub logs_title: &'static str,
    pub providers_title: &'static str,
    pub accounts_title: &'static str,
    pub help_title: &'static str,
    pub help_close: &'static str,
    pub empty_accounts_before: &'static str,
    pub empty_accounts_mid: &'static str,
    pub empty_accounts_after: &'static str,
    pub empty_providers_before: &'static str,
    pub empty_providers_after: &'static str,
    pub no_logs: &'static str,
    pub rename_prefix: &'static str,
    pub rename_suffix: &'static str,
    pub selected_for_batch: &'static str,
    pub selected_clear: &'static str,
    pub provider_line_1: &'static str,
    pub provider_line_2: &'static str,
    pub provider_line_3: &'static str,
    pub section_warmup: &'static str,
    pub auto_warmup: &'static str,
    pub hint_auto_warmup: &'static str,
}

pub fn t(language: Language) -> Copy {
    match language {
        Language::Zh => ZH,
        Language::En => EN,
    }
}

pub fn phrase(language: Language, english: &'static str) -> &'static str {
    if language != Language::Zh {
        return english;
    }
    match english {
        "fetching" => "正在获取",
        "marked" => "已标记",
        "sort" => "排序",
        "nav" => "移动",
        "search" => "搜索",
        "menu" => "菜单",
        "add" => "添加",
        "refresh" => "刷新",
        "detail" => "详情",
        "use" => "切换",
        "help" => "帮助",
        "quit" => "退出",
        "launch" => "启动",
        "previous" => "上一个",
        "next" => "下一个",
        "edit" => "编辑",
        "rename" => "重命名",
        "remove" => "删除",
        "save" => "保存",
        "up" => "向上",
        "down" => "向下",
        "latest" => "最新",
        "switch tabs" => "切换页",
        "edit / run" => "编辑 / 执行",
        "pull" => "拉取",
        "upload" => "上传",
        "move" => "选择",
        "move selection" => "移动选择",
        "open selected account actions" => "打开账号操作",
        "add new account" => "添加账号",
        "refresh visible accounts" => "刷新可见账号",
        "show / hide account detail panel" => "显示或隐藏详情",
        "use (switch to)" => "切换到该账号",
        "show this help (main view)" => "显示帮助",
        "quit (main view)" => "退出",
        "launch Codex" => "启动 Codex",
        "cycle sort (name / quota / status)" => "切换排序（名称 / 额度 / 状态）",
        "toggle mark" => "标记",
        "clear marks / search / popup" => "清除标记、搜索或弹窗",
        "refresh this account (in the Enter menu)" => "刷新此账号（在菜单中）",
        "re-login (in the Enter menu)" => "重新登录（在菜单中）",
        "rename (in the Enter menu)" => "重命名（在菜单中）",
        "warmup (in the Enter menu)" => "预热（在菜单中）",
        "use earliest reset card, asks to confirm (in the Enter menu)" => "使用最早到期的重置卡（会确认）",
        "delete (in the Enter menu)" => "删除（在菜单中）",
        "refresh selected" => "刷新已选",
        "warmup selected" => "预热已选",
        "re-login selected (sequential)" => "依次重新登录已选",
        "delete selected" => "删除已选",
        "move provider selection" => "移动提供方选择",
        "launch Codex (pick model, reasoning, extra args)" => "启动 Codex（选择模型、思考等级和额外参数）",
        "edit provider" => "编辑提供方",
        "rename provider" => "重命名提供方",
        "add provider" => "添加提供方",
        "remove selected provider" => "删除选中的提供方",
        "move field" => "移动字段",
        "edit or toggle the focused field" => "编辑或切换当前字段",
        "toggle boolean options" => "切换开关",
        "save config.toml" => "保存配置",
        "cancel the current field edit" => "取消当前编辑",
        "scroll session logs" => "滚动日志",
        "jump to latest log" => "跳到最新日志",
        "edit field, or pull / push the focused action" => "编辑字段，或执行拉取 / 上传",
        "pull this group" => "选择要下载的分组",
        "upload this group" => "选择要上传的分组",
        "choose groups to download" => "选择要下载的分组",
        "choose groups to upload" => "选择要上传的分组",
        "toggle auto-refresh" => "开关自动刷新",
        "click tabs/rows; double-click rows opens menus; wheel scrolls logs/help/menus" => "点击页签和行；双击行打开菜单；滚轮滚动日志、帮助和菜单",
        "next / previous tab (Accounts / Providers / Settings / Logs)" => "下一个 / 上一个页（账号 / 提供方 / 设置 / 日志）",
        "Navigation" => "导航",
        "Selection" => "选择",
        "Accounts tab" => "账号页",
        "Batch actions  (open via Enter when accounts marked)" => "批量操作（标记账号后按回车）",
        "Providers tab (Beta)" => "提供方页",
        "Settings tab" => "设置页",
        "Sync" => "同步",
        "Logs tab" => "日志页",
        "Global" => "全局",
        other => other,
    }
}

pub fn language_name(ui: Language, value: Language) -> &'static str {
    let text = t(ui);
    match value {
        Language::En => text.language_en,
        Language::Zh => text.language_zh,
    }
}

pub fn pushed(language: Language, group: &str, count: usize) -> String {
    match language {
        Language::En => format!(
            "Uploaded group {group} with {count} account(s). The remote group with this name was replaced."
        ),
        Language::Zh => format!("已上传分组 {group}，{count} 个账号。同名分组已被覆盖。"),
    }
}

pub fn uploaded(language: Language, groups: usize, accounts: usize, removed_remote: usize) -> String {
    match language {
        Language::En => {
            let mut message = format!(
                "Uploaded {groups} group(s), {accounts} account(s). Matching remote groups were replaced."
            );
            if removed_remote > 0 {
                message.push_str(&format!(
                    " Removed {removed_remote} remote group(s) that are not on this computer."
                ));
            }
            message
        }
        Language::Zh => {
            let mut message =
                format!("已上传 {groups} 个分组、{accounts} 个账号。线上同名分组已覆盖。");
            if removed_remote > 0 {
                message.push_str(&format!(
                    " 另删除了线上 {removed_remote} 个本机没有的分组。"
                ));
            }
            message
        }
    }
}

pub fn downloaded(
    language: Language,
    groups: usize,
    created: usize,
    updated: usize,
    removed: usize,
    skipped: usize,
) -> String {
    match language {
        Language::En => {
            let mut message = format!(
                "Downloaded {groups} group(s): {created} added, {updated} updated, {removed} removed."
            );
            if skipped > 0 {
                message.push_str(&format!(" Skipped {skipped}."));
            }
            message
        }
        Language::Zh => {
            let mut message =
                format!("已下载 {groups} 个分组，新增 {created}，更新 {updated}，移除 {removed}。");
            if skipped > 0 {
                message.push_str(&format!(" 跳过 {skipped} 个。"));
            }
            message
        }
    }
}

pub fn no_remote_groups(language: Language) -> String {
    match language {
        Language::En => "No remote groups".to_string(),
        Language::Zh => "线上没有分组".to_string(),
    }
}

pub fn pulled(language: Language, group: &str, created: usize, updated: usize, skipped: usize) -> String {
    match language {
        Language::En => {
            let mut message =
                format!("Pulled group {group}: {created} account(s) added, {updated} updated.");
            if skipped > 0 {
                message.push_str(&format!(" Skipped {skipped}."));
            }
            message
        }
        Language::Zh => {
            let mut message = format!("已拉取分组 {group}，新增 {created} 个账号，更新 {updated} 个。");
            if skipped > 0 {
                message.push_str(&format!(" 跳过 {skipped} 个。"));
            }
            message
        }
    }
}

pub fn missing_group(language: Language, group: &str) -> String {
    match language {
        Language::En => format!("No remote group named {group}"),
        Language::Zh => format!("线上没有分组 {group}"),
    }
}

pub fn unknown_schema(language: Language, schema: u32) -> String {
    match language {
        Language::En => format!("Unrecognized group format {schema}"),
        Language::Zh => format!("不认识的分组格式 {schema}"),
    }
}

pub fn profile_collision(language: Language, alias: &str) -> String {
    match language {
        Language::En => format!("{alias} is a ChatGPT account and was not replaced"),
        Language::Zh => format!("{alias} 已是 ChatGPT 账号，不能覆盖成提供方"),
    }
}

const EN: Copy = Copy {
    settings: " Settings ",
    settings_dirty: " Settings * ",
    section_language: "Language",
    section_network: "Proxy / network / TUI",
    section_selection: "Selection",
    section_launch: "Launch",
    language: "language",
    language_en: "English",
    language_zh: "中文",
    proxy_url: "proxy.url",
    proxy_no_proxy: "proxy.no_proxy",
    cache_ttl: "cache.ttl",
    max_concurrent: "network.max_concurrent",
    tui_refresh: "tui.auto_refresh_interval_secs",
    safety_margin: "use.safety_margin_7d",
    team_priority: "use.team_priority",
    restart_app_server: "use.restart_app_server",
    restore_delay: "launch.restore_delay_secs",
    hint_language: "Interface language. English is the default. Enter or left/right switches it.",
    hint_proxy_url: "Proxy for usage, login and provider requests (http, https, socks4, socks5, socks5h).\n--proxy and CS_PROXY override it; empty = none.",
    hint_proxy_no_proxy: "Hosts that bypass the proxy, comma-separated (NO_PROXY syntax); empty = none.",
    hint_cache_ttl: "Seconds a usage result is reused before it is fetched again (minimum 1).",
    hint_max_concurrent: "Maximum simultaneous usage requests (minimum 1).",
    hint_tui_refresh: "Seconds between refreshes while `t` auto-refresh is on (minimum 30).",
    hint_safety_margin: "7d headroom % below which auto-select penalizes an account.",
    hint_team_priority: "Prefer Team-plan accounts when auto-selecting.",
    hint_restart_app_server: "After the live auth.json changes (use, TUI u, login), restart a running Codex\napp-server daemon so new sessions use the account. Interrupts turns in progress.\noff = leave the daemon alone and only show the manual command.",
    hint_restore_delay: "Seconds `launch` waits before restoring auth.json after starting Codex\n(minimum 1; 0 would restore it before Codex has read it).",
    empty: "(empty)",
    on: "on",
    off: "off",
    settings_help: "click field  j/k move  enter/space edit or toggle  ←/→ toggle  s save  esc cancel edit",
    saved: "Saved config.toml",
    sync_tab: " Sync ",
    sync_title: " Sync ",
    sync_intro_1: "Upload and download open the group list. Enter runs the checked groups. a runs every group.",
    sync_intro_2: "Accepts redis://host:port, Upstash rediss://, or an Upstash https:// REST URL.",
    sync_group: "Group",
    sync_url: "Redis URL",
    sync_password: "Password",
    sync_unset: "not set",
    sync_set: "set",
    sync_pull: "Pull     choose remote groups and replace those local groups",
    sync_push: "Upload   choose local groups and replace those remote groups",
    sync_help: "↑↓ move    Enter edit or open groups    p pull    u upload",
    picker_upload_title: " Upload groups ",
    picker_download_title: " Download groups ",
    picker_upload_help: "space check   enter upload checked   a upload all and replace every remote group   esc cancel",
    picker_download_help: "space check   enter download checked   a download all   esc cancel",
    sync_busy: "Connecting…",
    sync_saved: "Saved on this computer",
    all_accounts: " All ",
    footer_choose: "move",
    footer_edit: "edit / run",
    footer_pull: "pull",
    footer_upload: "upload",
    tab_accounts: "Accounts",
    tab_providers: "Providers (Beta)",
    tab_settings: "Settings",
    tab_logs: "Logs",
    tab_hint: "Tab to switch",
    col_alias: "Alias",
    col_email: "Email",
    col_plan: "Plan",
    col_status: "Status",
    col_cards: "Cards",
    col_expires: "Expires",
    col_credits: "Credits",
    col_models: "Models",
    col_base_url: "Base URL",
    logs_title: " Session logs ",
    providers_title: " Custom providers (Beta) ",
    accounts_title: " codex-switch ",
    help_title: "Help",
    help_close: "  esc / q / h to close • j k arrows / PgUp PgDn / mouse wheel to scroll",
    empty_accounts_before: "No accounts yet. Press ",
    empty_accounts_mid: " to add one, or ",
    empty_accounts_after: " to quit.",
    empty_providers_before: "No custom providers. Press ",
    empty_providers_after: " to add one.",
    no_logs: "No logs in this session.",
    rename_prefix: " Rename: ",
    rename_suffix: "  (Enter confirm / Esc cancel)",
    selected_for_batch: "for batch",
    selected_clear: "to clear",
    provider_line_1: "Manage Responses-compatible API endpoints and multiple models.",
    provider_line_2: "Set reasoning/web_search per model; choose a model to launch Codex.",
    provider_line_3: "No ChatGPT quotas or automatic account selection.",
    section_warmup: "Warmup",
    auto_warmup: "auto warmup",
    hint_auto_warmup: "While this window is open, warm an account one minute after its 5h or 7d window resets.",
};

const ZH: Copy = Copy {
    settings: " 设置 ",
    settings_dirty: " 设置 * ",
    section_language: "语言",
    section_network: "代理 / 网络 / 界面",
    section_selection: "选号",
    section_launch: "启动",
    language: "语言",
    language_en: "English",
    language_zh: "中文",
    proxy_url: "代理地址",
    proxy_no_proxy: "不走代理的主机",
    cache_ttl: "额度缓存秒数",
    max_concurrent: "并发请求数",
    tui_refresh: "自动刷新间隔",
    safety_margin: "7 天安全余量",
    team_priority: "优先 Team 计划",
    restart_app_server: "切换后重启服务",
    restore_delay: "启动后恢复延迟",
    hint_language: "界面语言。默认英语。回车或左右键切换。",
    hint_proxy_url: "额度、登录和提供方请求使用的代理（http、https、socks4、socks5、socks5h）。\n--proxy 和 CS_PROXY 优先；留空表示不用代理。",
    hint_proxy_no_proxy: "不走代理的主机，逗号分隔（NO_PROXY 写法）；留空表示没有。",
    hint_cache_ttl: "额度结果在再次请求前保留的秒数（最小 1）。",
    hint_max_concurrent: "同时查询额度的最大请求数（最小 1）。",
    hint_tui_refresh: "`t` 打开自动刷新后，两次刷新之间的秒数（最小 30）。",
    hint_safety_margin: "7 天剩余比例低于这个值时，自动选号会降低该账号的分数。",
    hint_team_priority: "自动选号时优先使用 Team 计划账号。",
    hint_restart_app_server: "活的 auth.json 变化后（use、界面里的 u、login），重启正在运行的 Codex\napp-server，让新会话使用该账号。会打断进行中的回合。\n关 = 不重启，只提示手动命令。",
    hint_restore_delay: "`launch` 启动 Codex 后、恢复原来的 auth.json 之前等待的秒数\n（最小 1；0 会在 Codex 读到凭据之前就恢复）。",
    empty: "（空）",
    on: "开",
    off: "关",
    settings_help: "点击字段  j/k 移动  回车或空格编辑/切换  ←/→ 切换  s 保存  esc 取消编辑",
    saved: "已保存 config.toml",
    sync_tab: " 同步 ",
    sync_title: " 同步 ",
    sync_intro_1: "上传和下载都会先列出分组。回车处理勾选的组，a 处理全部。",
    sync_intro_2: "支持 redis://主机:端口、Upstash 的 rediss://，或 https:// REST 地址。",
    sync_group: "分组名",
    sync_url: "Redis 地址",
    sync_password: "密码",
    sync_unset: "未填写",
    sync_set: "已填写",
    sync_pull: "拉取     选择线上分组，覆盖同步到本机",
    sync_push: "上传     选择本机分组，覆盖线上同名分组",
    sync_help: "↑↓ 选择    Enter 编辑或打开分组    p 拉取    u 上传",
    picker_upload_title: " 上传分组 ",
    picker_download_title: " 下载分组 ",
    picker_upload_help: "空格勾选   回车上传勾选   a 全部上传并覆盖线上所有分组   esc 取消",
    picker_download_help: "空格勾选   回车下载勾选   a 下载全部并覆盖本机对应分组   esc 取消",
    sync_busy: "正在连接…",
    sync_saved: "已保存到本机",
    all_accounts: " 全部 ",
    footer_choose: "选择",
    footer_edit: "编辑 / 执行",
    footer_pull: "拉取",
    footer_upload: "上传",
    tab_accounts: "账号",
    tab_providers: "提供方",
    tab_settings: "设置",
    tab_logs: "日志",
    tab_hint: "Tab 切换",
    col_alias: "别名",
    col_email: "邮箱",
    col_plan: "计划",
    col_status: "状态",
    col_cards: "重置卡",
    col_expires: "到期",
    col_credits: "额度点",
    col_models: "模型",
    col_base_url: "接口地址",
    logs_title: " 会话日志 ",
    providers_title: " 自定义提供方 ",
    accounts_title: " codex-switch ",
    help_title: "帮助",
    help_close: "  esc / q / h 关闭 • j k 方向键 / PgUp PgDn / 滚轮滚动",
    empty_accounts_before: "还没有账号。按 ",
    empty_accounts_mid: " 添加，或按 ",
    empty_accounts_after: " 退出。",
    empty_providers_before: "还没有自定义提供方。按 ",
    empty_providers_after: " 添加。",
    no_logs: "这次会话还没有日志。",
    rename_prefix: " 重命名: ",
    rename_suffix: "  （回车确认 / Esc 取消）",
    selected_for_batch: "批量操作",
    selected_clear: "清除",
    provider_line_1: "管理兼容 Responses 的接口和多个模型。",
    provider_line_2: "为每个模型设置思考等级和 web_search，并选择模型启动 Codex。",
    provider_line_3: "不显示 ChatGPT 额度，也不参与自动选号。",
    section_warmup: "预热",
    auto_warmup: "自动预热",
    hint_auto_warmup: "本窗口打开时，账号的 5 小时或 7 天窗口重置满 1 分钟后自动预热一次。",
};
