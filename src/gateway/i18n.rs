//! Core i18n — lightweight message catalog for user-facing system messages.
//!
//! Uses the same language codes as the Panel (`en`, `zh-Hans`).
//! Locale is resolved from `GeneralConfig.language` at runtime.
//!
//! # What belongs in here, and what does not
//!
//! Only bytes a **human reads**. A tool error, a deny reason, a `withheld`
//! explanation and a plan verdict sentence all read like prose but their
//! audience is the model, and translating those would make the model's input
//! depend on a display setting — the one thing a prompt must not do. The split
//! runs straight through single files: `clarification::render` builds the reply
//! hint a person reads (translated) while `clarification::ask` builds the
//! refusal the model reads (never translated).

use std::sync::OnceLock;

use crate::orchestrator::dispatch::TerminateReason;
use crate::spend::Limit;

/// Supported locales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    Zh,
}

impl Locale {
    /// Parse a locale string from config. Falls back to `Zh` for any Chinese variant.
    #[must_use]
    pub fn from_config(s: Option<&str>) -> Self {
        match s {
            Some(s) if s.starts_with("en") => Self::En,
            _ => Self::Zh, // default to Chinese (matches current behavior)
        }
    }

    /// Resolve from a run's `metadata["locale"]` stamp (written by the inbound
    /// router from `[general] language`). Absent on Panel / CLI / cron runs →
    /// `Zh`, byte-identical to the prior hardcoded behaviour.
    #[must_use]
    pub fn from_run_metadata(metadata: &std::collections::HashMap<String, String>) -> Self {
        Self::from_config(metadata.get("locale").map(String::as_str))
    }
}

/// The process-wide UI locale, installed once at server boot.
///
/// # Why a process global rather than a parameter
///
/// The strings this serves are built deep inside code with no run in hand: the
/// clarification renderer is called from the tool that asks (`ask`), from the
/// registry that advances the cursor after a reply (`session::resolve_many`),
/// and from the inbound router. Threading a `Locale` to all three would put the
/// setting on every constructor between here and there, and the day one of them
/// is added without it, that path silently reverts to English — the same shape
/// as `SecretMasker`'s seven construction sites, where the fix was to install
/// the configuration on the *type* so a call site that has never heard of it
/// still inherits it.
///
/// It is also not lossy **today**: both existing consumers resolve the locale
/// from the one `[general] language` key at boot, and the per-run
/// `metadata["locale"]` stamp is written from that same key. If a per-session
/// language ever lands, this becomes a parameter and the compiler will find
/// every reader.
///
/// # Changing the language takes a restart, and that is the declared contract
///
/// `OnceLock` means an edit to `[general] language` does not reach these
/// strings until the process restarts. That matches what the config system
/// already promises: `general` is **not** in `reload_impact::LIVE_SECTIONS`
/// (`route` / `behavior` / `execution`), so `ReloadImpact::classify` reports
/// `Restart` for it and `self_config` tells the user so. Note the gateway's
/// *other* locale readers happen to be more live than that promise — they
/// re-read config per run — so a hot language change moves them and not this.
/// If `general` is ever promoted to a live section, this global has to become
/// a per-run parameter in the same change.
static INSTALLED_LOCALE: OnceLock<Locale> = OnceLock::new();

/// Install the process-wide UI locale. First call wins; later calls are
/// ignored, which is why boot resolves it from one config read.
pub fn install_locale(locale: Locale) {
    let _ = INSTALLED_LOCALE.set(locale);
}

/// Translate a **human-facing** message using the installed locale.
///
/// Falls back to [`Locale::En`] when nothing installed one — unit tests, CLI
/// subcommands, and any embedding that never booted a gateway. That default is
/// deliberate and is *not* [`Locale::from_config`]'s `Zh`: the strings this
/// serves were English literals before they were catalog entries, so an
/// uninstalled process keeps emitting the same bytes it always did.
///
/// Never use this for text the model reads — see the module docs.
#[must_use]
pub fn t_ui(msg: Msg<'_>) -> String {
    t(msg, INSTALLED_LOCALE.get().copied().unwrap_or(Locale::En))
}

// ---------------------------------------------------------------------------
// Message keys
// ---------------------------------------------------------------------------

/// All user-facing system messages that need translation.
///
/// `Copy` so a key can sit in a `const` table next to the wire value it labels
/// (`scratchpad::APPROVAL_CHOICES`) — that pairing is what keeps a translated
/// label from drifting away from the untranslated value it stands for.
#[derive(Clone, Copy)]
pub enum Msg<'a> {
    // --- /new command ---
    NewSessionStarted {
        topic_suffix: &'a str,
    },

    // --- /stop command ---
    RunStopped,
    NoActiveRun,

    // --- execution errors (single bucket set, see `ReceiptKind`) ---
    ErrReceipt(ReceiptKind),

    // --- busy queue (§4.8) ---
    QueuedMessagesDropped {
        count: usize,
    },

    // --- `/btw promote` ---
    /// The side answer crossed. Names the question rather than echoing the
    /// answer: the answer itself has just been appended to the transcript the
    /// user is looking at, and repeating it here would print it twice.
    BtwPromoted {
        question: &'a str,
    },
    /// There was no completed side exchange to carry over. A true answer, not
    /// a failure — which is why it is here and not in [`ReceiptKind`].
    BtwNothingToPromote,

    // --- topic generation prompt (sent to LLM, not shown to user) ---
    TopicGenerationPrompt {
        conversation: &'a str,
    },

    // --- clarification card (`src/clarification/render.rs`) ---
    /// Reply hint under a question with no choices.
    ClarifyReplyFreeText,
    /// Reply hint under a pick-one menu.
    ClarifyReplyPickOne,
    /// Reply hint under a pick-several menu.
    ClarifyReplyPickMany,

    // --- plan-approval card (`scratchpad(action='request_approval')`) ---
    /// Verdict label: approve as written.
    PlanApproveLabel,
    /// Verdict label: approve with changes, described in free text.
    PlanReviseLabel,
    /// Verdict label: do not do this.
    PlanRejectLabel,
    /// Lead-in for the plan's objective line.
    PlanObjectiveLead,
    /// The question itself, under the rendered plan.
    PlanApprovePrompt,
}

/// The user-facing classification of a failed run — the **single** bucket set
/// behind every error the gateway shows a human.
///
/// Two producers feed it and they must never drift again:
/// * [`crate::gateway::execution_engine::ExecutionError::receipt_kind`] maps the
///   typed variant directly, and
/// * [`classify_error_text`] recovers a bucket from the one string-carrying
///   variant (`ExecutionError::Failed`), whose payload comes from the dispatch
///   loop *after* failover is exhausted.
///
/// Before this existed the engine carried its own hardcoded-Chinese classifier
/// (`classify_failed` / `is_rate_limited` / `is_unreachable`) while this module
/// carried a second, localized one — so the Panel showed Chinese under
/// `language = "en"`, an expired API key was reported as "just retry" (the
/// engine had no auth bucket), and the engine's naive `contains("connection")`
/// matched `disconnection_policy` (exactly the foot-gun [`contains_phrase`]
/// was written to avoid).
///
/// `Eq` is deliberately absent (only `PartialEq`): [`SpendExhausted`](Self::SpendExhausted)
/// carries a [`Limit`], and `Limit::PerUser`'s two `f64` fields cannot
/// implement `Eq`. `Copy` stays — `Limit` is itself `Copy` (all-`f64`
/// fields), so nothing here forces a move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReceiptKind {
    /// The run exceeded its wall-clock budget.
    Timeout,
    /// The user (or an `Interrupt`-mode message) stopped the run.
    Cancelled,
    /// The session already has an in-flight run and the message could not be
    /// steered or queued.
    AgentBusy,
    /// Every provider in the chain reported rate limiting.
    RateLimited,
    /// Credential rejected (401 / invalid API key) — retrying will not help.
    Auth,
    /// Network / upstream outage across the whole provider chain.
    Unreachable,
    /// Anything else. Deliberately opaque: the raw chain goes to the server
    /// log, never onto the wire (hermes GAP-5).
    Failed,
    /// `principal`'s spend ceiling was reached — see `spend::check`. Carries
    /// the [`Limit`] that fired (wording is chosen by *its* shape — see
    /// `Limit::PerUser`/`Limit::Total`'s own docs — never by asking "may this
    /// caller see that number", a question nothing at render time can
    /// answer) and the reset instant `spend::check` computed for this exact
    /// denial. That instant is carried through unchanged from
    /// `ExecutionError::SpendExhausted` — never recomputed here or at
    /// `receipt_kind()` time — because a fresh `period_end_ms(Utc::now(),
    /// ..)` call moments later can name the *next* window's end (if
    /// rendering crosses a period boundary, or the run was parked and
    /// retried) or read a since-reloaded `current_policy()` (spend policy
    /// is hot-swappable via `spend::update_policy`). See
    /// `ExecutionError::SpendExhausted`'s own doc for the full argument.
    SpendExhausted { limit: Limit, reset_ms: i64 },
}

impl ReceiptKind {
    /// Stable wire code carried on `StreamEvent::RunError.error_code`.
    /// Clients may switch on it, so these strings are API — do not rename.
    ///
    /// The spellings live in `aleph_protocol::receipt::ReceiptCode`, which the
    /// Panel also reads, so the two sides cannot drift into two taxonomies.
    #[must_use]
    pub const fn code(self) -> &'static str {
        self.protocol_code().as_wire()
    }

    /// This bucket as the shared protocol type.
    #[must_use]
    pub const fn protocol_code(self) -> aleph_protocol::receipt::ReceiptCode {
        use aleph_protocol::receipt::ReceiptCode as C;
        match self {
            Self::Timeout => C::Timeout,
            Self::Cancelled => C::Cancelled,
            Self::AgentBusy => C::AgentBusy,
            Self::RateLimited => C::RateLimited,
            Self::Auth => C::Auth,
            Self::Unreachable => C::ProvidersUnreachable,
            Self::Failed => C::Failed,
            Self::SpendExhausted { .. } => C::SpendExhausted,
        }
    }

    /// Worth parking-and-retrying (the two provider-hiccup buckets), as
    /// opposed to terminal.
    ///
    /// The single predicate `execute.rs::park_loop_on_transient_failure` and
    /// `goal_continuation.rs::block_goal_on_failure` both call — before this
    /// existed each hand-wrote `matches!(kind, RateLimited | Unreachable)`,
    /// and two hand-written copies is exactly how one of them would end up
    /// answering `SpendExhausted` differently from the other. It must not: a
    /// spend ceiling does not lift until the period resets, a park costs no
    /// iteration (see `park_loop_on_transient_failure`'s own doc), and
    /// nothing else bounds how many times a tick can re-fire against a wall
    /// that has not moved — parking it would be an infinite billing-denial
    /// loop, not a bounded retry.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::RateLimited | Self::Unreachable)
    }
}

/// Recover a [`ReceiptKind`] from a failure string.
///
/// Used for `ExecutionError::Failed`, whose payload originates in the gateway
/// dispatch loop *after* failover and retries are exhausted — so a rate-limit
/// or network signature here means every provider/model in the chain was tried
/// and the failure is genuinely transient.
///
/// Matching is whole-token via [`contains_phrase`]; substring matching would
/// let `disconnection_policy` read as a network outage.
#[must_use]
pub fn classify_error_text(error: &str) -> ReceiptKind {
    let lower = error.to_lowercase();
    if contains_phrase(
        &lower,
        &[
            "429",
            "too many requests",
            "rate limit",
            "rate_limit",
            "usage_limit",
            "receiving too many requests",
        ],
    ) {
        ReceiptKind::RateLimited
    } else if contains_phrase(&lower, &["401", "403", "unauthorized", "invalid api key"]) {
        ReceiptKind::Auth
    } else if contains_phrase(&lower, &["timeout", "timed out"]) {
        ReceiptKind::Timeout
    } else if contains_phrase(
        &lower,
        &[
            "network error",
            "error sending request",
            "connection",
            "connection refused",
            "dns",
            "500",
            "502",
            "503",
            "internal server error",
            "service unavailable",
        ],
    ) {
        ReceiptKind::Unreachable
    } else {
        ReceiptKind::Failed
    }
}

/// Translate a message key to the target locale.
#[must_use]
pub fn t(msg: Msg<'_>, locale: Locale) -> String {
    match (msg, locale) {
        // ============================================================
        // /new command
        // ============================================================
        (Msg::NewSessionStarted { topic_suffix }, Locale::Zh) => {
            format!("新对话已开始{topic_suffix}")
        }
        (Msg::NewSessionStarted { topic_suffix }, Locale::En) => {
            format!("New conversation started{topic_suffix}")
        }

        // ============================================================
        // /stop command
        // ============================================================
        (Msg::RunStopped, Locale::Zh) => "⏹ 已停止当前任务。".into(),
        (Msg::RunStopped, Locale::En) => "⏹ Stopped the current task.".into(),
        (Msg::NoActiveRun, Locale::Zh) => "当前没有正在执行的任务。".into(),
        (Msg::NoActiveRun, Locale::En) => "No task is currently running.".into(),

        // ============================================================
        // Execution errors
        // ============================================================
        (Msg::ErrReceipt(ReceiptKind::Timeout), Locale::Zh) => {
            "任务超时未完成。请重试，或把任务拆小一些再发给我。".into()
        }
        (Msg::ErrReceipt(ReceiptKind::Timeout), Locale::En) => {
            "The task timed out. Retry, or break it into smaller pieces.".into()
        }

        (Msg::ErrReceipt(ReceiptKind::Cancelled), Locale::Zh) => "任务已取消。".into(),
        (Msg::ErrReceipt(ReceiptKind::Cancelled), Locale::En) => "Task cancelled.".into(),

        (Msg::ErrReceipt(ReceiptKind::AgentBusy), Locale::Zh) => {
            "当前 Agent 正忙，请稍后再试。".into()
        }
        (Msg::ErrReceipt(ReceiptKind::AgentBusy), Locale::En) => {
            "The agent is busy right now. Please try again shortly.".into()
        }

        (Msg::ErrReceipt(ReceiptKind::RateLimited), Locale::Zh) => {
            "模型服务商当前限流（请求过于频繁），且备用通道也已用尽。稍等片刻后重试即可。".into()
        }
        (Msg::ErrReceipt(ReceiptKind::RateLimited), Locale::En) => {
            "The model provider is rate limiting and every fallback channel is exhausted. \
             Wait a moment and retry."
                .into()
        }

        (Msg::ErrReceipt(ReceiptKind::Auth), Locale::Zh) => {
            "模型服务认证失败（API Key 无效或已过期），重试不会有帮助。请检查 API Key 配置。".into()
        }
        (Msg::ErrReceipt(ReceiptKind::Auth), Locale::En) => {
            "Model service authentication failed (invalid or expired API key) — retrying \
             will not help. Please check your API key."
                .into()
        }

        (Msg::ErrReceipt(ReceiptKind::Unreachable), Locale::Zh) => {
            "无法连接到模型 / 搜索服务（网络不可达或服务暂时中断），所有可用通道均已尝试。\
             请检查网络后重试。"
                .into()
        }
        (Msg::ErrReceipt(ReceiptKind::Unreachable), Locale::En) => {
            "Could not reach the model / search service (network unreachable or upstream \
             outage); every available channel was tried. Check your connection and retry."
                .into()
        }

        (Msg::ErrReceipt(ReceiptKind::Failed), Locale::Zh) => "任务执行失败，请重试。".into(),
        (Msg::ErrReceipt(ReceiptKind::Failed), Locale::En) => {
            "The task failed. Please try again.".into()
        }

        // Wording is chosen by `Limit`'s SHAPE, never by asking who is
        // asking: `PerUser` renders both numbers because they are the
        // caller's own; `Total` renders none — see `Limit::Total`'s doc.
        // G12 pins the `Total` arms carrying no dollar figure.
        (
            Msg::ErrReceipt(ReceiptKind::SpendExhausted {
                limit: Limit::PerUser { spent, limit },
                reset_ms,
            }),
            Locale::Zh,
        ) => {
            format!(
                "本周期个人支出已达上限：已花费 ${spent:.2}，上限 ${limit:.2}。将于 {} 重置。",
                format_reset_instant(reset_ms)
            )
        }
        (
            Msg::ErrReceipt(ReceiptKind::SpendExhausted {
                limit: Limit::PerUser { spent, limit },
                reset_ms,
            }),
            Locale::En,
        ) => {
            format!(
                "Spend ceiling reached: ${spent:.2} spent against a ${limit:.2} per-user limit \
                 for this period. Resets at {}.",
                format_reset_instant(reset_ms)
            )
        }
        (
            Msg::ErrReceipt(ReceiptKind::SpendExhausted {
                limit: Limit::Total,
                reset_ms,
            }),
            Locale::Zh,
        ) => {
            format!(
                "本机共享支出额度已用尽，本周期暂时无法继续。将于 {} 重置。",
                format_reset_instant(reset_ms)
            )
        }
        (
            Msg::ErrReceipt(ReceiptKind::SpendExhausted {
                limit: Limit::Total,
                reset_ms,
            }),
            Locale::En,
        ) => {
            format!(
                "This machine's shared spend limit for the period has been reached. Resets at {}.",
                format_reset_instant(reset_ms)
            )
        }

        (Msg::QueuedMessagesDropped { count }, Locale::Zh) => {
            format!("已随本次停止取消 {count} 条排队中的消息。")
        }
        (Msg::QueuedMessagesDropped { count }, Locale::En) => {
            format!("Also dropped {count} queued message(s) waiting on this session.")
        }

        // ============================================================
        // `/btw promote`
        // ============================================================
        (Msg::BtwPromoted { question }, Locale::Zh) => {
            format!("已把这个旁问的回答带进本对话：{question}")
        }
        (Msg::BtwPromoted { question }, Locale::En) => {
            format!("Promoted the side answer into this conversation: {question}")
        }
        (Msg::BtwNothingToPromote, Locale::Zh) => {
            "这个对话里还没有已经答完的旁问，暂时没有可以带过来的内容。".into()
        }
        (Msg::BtwNothingToPromote, Locale::En) => {
            "There is no completed side question in this conversation yet, so there is \
             nothing to promote."
                .into()
        }

        // ============================================================
        // Topic generation prompt (sent to LLM, language-adaptive)
        // ============================================================
        (Msg::TopicGenerationPrompt { conversation }, Locale::Zh) => {
            format!(
                "用一句简短的中文概括以下对话的主题（10字以内，不要标点符号）：\n\n{conversation}"
            )
        }
        (Msg::TopicGenerationPrompt { conversation }, Locale::En) => {
            format!(
                "Summarize the topic of the following conversation in a short English phrase \
                 (under 10 words, no punctuation):\n\n{conversation}"
            )
        }

        // ============================================================
        // Clarification card
        //
        // The English arms are byte-identical to the literals these
        // replaced, so an install that never calls `install_locale`
        // renders exactly what it rendered before the catalog existed.
        // ============================================================
        (Msg::ClarifyReplyFreeText, Locale::En) => "Reply with your answer.".into(),
        (Msg::ClarifyReplyFreeText, Locale::Zh) => "请回复你的答案。".into(),
        (Msg::ClarifyReplyPickOne, Locale::En) => "Reply with the number or your answer.".into(),
        (Msg::ClarifyReplyPickOne, Locale::Zh) => "请回复序号，或直接写出你的答案。".into(),
        (Msg::ClarifyReplyPickMany, Locale::En) => {
            "Reply with the numbers separated by commas, or your own answer.".into()
        }
        (Msg::ClarifyReplyPickMany, Locale::Zh) => {
            "请回复多个序号（用逗号分隔），或直接写出你的答案。".into()
        }

        // ============================================================
        // Plan-approval card
        //
        // Only the LABELS are translated. Each option's `value` stays the
        // English key (`approved` / `revise` / `rejected`) because that is
        // what `verdict_of` reads and what the model is told it received —
        // translating it would make the verdict depend on a display
        // setting.
        // ============================================================
        (Msg::PlanApproveLabel, Locale::En) => "Approve — start working the plan".into(),
        (Msg::PlanApproveLabel, Locale::Zh) => "批准 — 按这份计划开始执行".into(),
        (Msg::PlanReviseLabel, Locale::En) => "Revise — tell me what to change".into(),
        (Msg::PlanReviseLabel, Locale::Zh) => "修改 — 告诉我要改什么".into(),
        (Msg::PlanRejectLabel, Locale::En) => "Reject — do not do this".into(),
        (Msg::PlanRejectLabel, Locale::Zh) => "否决 — 不要做这件事".into(),
        (Msg::PlanObjectiveLead, Locale::En) => "Objective".into(),
        (Msg::PlanObjectiveLead, Locale::Zh) => "目标".into(),
        (Msg::PlanApprovePrompt, Locale::En) => "Approve this plan?".into(),
        (Msg::PlanApprovePrompt, Locale::Zh) => "是否批准这份计划？".into(),
    }
}

/// Render a `ReceiptKind::SpendExhausted` reset instant as RFC3339 — a
/// machine-parseable timestamp rather than free text, in both locales; the
/// same choice `metering::spend_denied_message` already made for the floor
/// arm's (model-facing, unlocalized) message. Falls back to the raw
/// millisecond value only if the timestamp is out of `chrono`'s
/// representable range — defensive only: `reset_ms` is always
/// `spend::period::period_end_ms`'s return value, which never produces one.
fn format_reset_instant(reset_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(reset_ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| reset_ms.to_string())
}

// ---------------------------------------------------------------------------
// Agent loop halt — render a cause-specific message per TerminateReason.
//
// Replaces the previous single `ErrLoopExhausted` blob, which always told
// the user to raise `[execution] max_iterations` — wrong advice for 7 of
// the 9 cap-style terminate reasons. Each branch now points at the actual
// root cause (consecutive tool failures, context budget, stall watchdog…)
// so the user sees actionable guidance instead of a misleading generic.
// ---------------------------------------------------------------------------

#[must_use]
pub fn render_loop_halt(
    reason: &TerminateReason,
    iterations: usize,
    tool_calls: usize,
    locale: Locale,
) -> String {
    match (reason, locale) {
        (TerminateReason::HitMaxIterations { used }, Locale::Zh) => format!(
            "抱歉，迭代上限 ({used}) 已用尽（共 {tool_calls} 次工具调用）。\n\
             如确需更多步骤，请调高 `[execution] max_iterations`，\
             或将任务拆分得更小、更具体。"
        ),
        (TerminateReason::HitMaxIterations { used }, Locale::En) => format!(
            "Sorry, hit the iteration cap ({used} iterations, {tool_calls} tool calls).\n\
             If more steps are needed, raise `[execution] max_iterations`, \
             or break the task into smaller, more specific steps."
        ),

        (TerminateReason::ConsecutiveFailureCap { consecutive }, Locale::Zh) => format!(
            "抱歉，连续 {consecutive} 轮工具调用全部失败（共 {iterations} 次迭代，\
             {tool_calls} 次工具调用）。\n\
             请检查工具配置或网络可达性。"
        ),
        (TerminateReason::ConsecutiveFailureCap { consecutive }, Locale::En) => format!(
            "Sorry, {consecutive} consecutive turns failed with tool errors \
             ({iterations} iterations, {tool_calls} tool calls).\n\
             Please check tool config or network reachability."
        ),

        (TerminateReason::ContextBudgetExhausted, Locale::Zh) => format!(
            "抱歉，上下文预算已用尽（{iterations} 次迭代后），无法继续。\n\
             请精简对话或新开会话。"
        ),
        (TerminateReason::ContextBudgetExhausted, Locale::En) => format!(
            "Sorry, the context budget was exhausted after {iterations} iterations.\n\
             Please trim the conversation or start a new session."
        ),

        (TerminateReason::StallTimeout { elapsed_ms }, Locale::Zh) => format!(
            "抱歉，{elapsed_ms} ms 内无任何进展被熔断（已执行 {iterations} 次迭代，\
             {tool_calls} 次工具调用）。\n\
             请检查工具是否阻塞或网络是否中断。"
        ),
        (TerminateReason::StallTimeout { elapsed_ms }, Locale::En) => format!(
            "Sorry, no progress for {elapsed_ms} ms — circuit-broken \
             ({iterations} iterations, {tool_calls} tool calls completed).\n\
             Check whether a tool is blocking or the network is stalled."
        ),

        (TerminateReason::TurnTimeout { phase, elapsed_ms }, Locale::Zh) => format!(
            "抱歉，单轮 `{phase}` 阶段 {elapsed_ms} ms 超时（已执行 {iterations} 次迭代）。"
        ),
        (TerminateReason::TurnTimeout { phase, elapsed_ms }, Locale::En) => format!(
            "Sorry, single-turn `{phase}` phase timed out at {elapsed_ms} ms \
             ({iterations} iterations completed)."
        ),

        (TerminateReason::VerifierVeto { vetos }, Locale::Zh) => {
            format!("抱歉，被验证器连续否决 {vetos} 次后退出（{iterations} 次迭代）。")
        }
        (TerminateReason::VerifierVeto { vetos }, Locale::En) => {
            format!("Sorry, exited after {vetos} verifier vetoes ({iterations} iterations).")
        }

        (TerminateReason::EmptyResponseExhausted, Locale::Zh) => format!(
            "抱歉，模型连续返回空响应（{iterations} 次迭代），无法继续。\n\
             这可能是上游提供商瞬时故障——请稍后重试。"
        ),
        (TerminateReason::EmptyResponseExhausted, Locale::En) => format!(
            "Sorry, the model returned empty responses for {iterations} iterations.\n\
             This may be an upstream provider issue — please try again later."
        ),

        (TerminateReason::StopHookHalt { reason }, Locale::Zh) => {
            format!("抱歉，Stop hook 拦截：{reason}。")
        }
        (TerminateReason::StopHookHalt { reason }, Locale::En) => {
            format!("Sorry, halted by Stop hook: {reason}.")
        }

        (TerminateReason::MaxOutputTokensExhausted, Locale::Zh) => format!(
            "抱歉，输出 token 上限耗尽（{iterations} 次迭代，{tool_calls} 次工具调用）。\n\
             请缩短请求或调高 `max_output_tokens`。"
        ),
        (TerminateReason::MaxOutputTokensExhausted, Locale::En) => format!(
            "Sorry, output token budget exhausted ({iterations} iterations, \
             {tool_calls} tool calls).\n\
             Shorten the request or raise `max_output_tokens`."
        ),

        (TerminateReason::DiminishingReturns, Locale::Zh) => format!(
            "检测到收益递减，已提前收尾（{iterations} 次迭代，{tool_calls} 次工具调用）。\n\
             如需继续，请把任务拆分得更具体后重试。"
        ),
        (TerminateReason::DiminishingReturns, Locale::En) => format!(
            "Stopped early on diminishing returns ({iterations} iterations, \
             {tool_calls} tool calls).\n\
             Break the task into more specific steps and retry if needed."
        ),

        // BudgetExhaustedPartialResult is the partial-result escalation
        // of a budget cap. The user-facing message highlights that the
        // partial work has been preserved for the next run (cron resume).
        // `reason` carries the underlying cap label ("hit_max_iterations",
        // "context_budget_exhausted", "max_output_tokens_exhausted") for
        // diagnostic surfacing.
        (TerminateReason::BudgetExhaustedPartialResult { reason, .. }, Locale::Zh) => format!(
            "抱歉，预算已用尽（{reason}），但已保留部分结果供下次续跑\
             （{iterations} 次迭代，{tool_calls} 次工具调用）。"
        ),
        (TerminateReason::BudgetExhaustedPartialResult { reason, .. }, Locale::En) => format!(
            "Sorry, the budget was exhausted ({reason}); partial result \
             preserved for resume ({iterations} iterations, \
             {tool_calls} tool calls)."
        ),

        (TerminateReason::ReactiveCompactExhausted, Locale::Zh) => format!(
            "抱歉，上下文已超出窗口，自动压缩后仍无法恢复（{iterations} 次迭代）。\n\
             请精简历史或新开会话。"
        ),
        (TerminateReason::ReactiveCompactExhausted, Locale::En) => format!(
            "Sorry, the prompt exceeded the model's context window and reactive \
             compaction did not recover it ({iterations} iterations).\n\
             Trim the conversation history or start a new session."
        ),

        // Completed and Cancelled should never reach here — callers gate on
        // `is_hit_limit()`. Fall back to a generic so we never panic in prod.
        // (`TerminateReason` is `#[non_exhaustive]` for downstream crates but
        // exhaustive within this crate — a new variant added in dispatch.rs
        // will force this match to be updated.)
        (TerminateReason::Completed | TerminateReason::Cancelled, Locale::Zh) => {
            format!("抱歉，会话提前结束（{iterations} 次迭代，{tool_calls} 次工具调用）。")
        }
        (TerminateReason::Completed | TerminateReason::Cancelled, Locale::En) => format!(
            "Sorry, the session ended early ({iterations} iterations, \
             {tool_calls} tool calls)."
        ),
    }
}

/// True if `lower` contains ANY of the supplied phrases as a whole-token
/// substring — i.e. preceded by a non-alphanumeric (or start of string) AND
/// followed by a non-alphanumeric (or end of string). This avoids the
/// `lower.contains("connection")` foot-gun where "disconnection_policy"
/// would match.
fn contains_phrase(lower: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| {
        let needle = *p;
        let bytes = lower.as_bytes();
        let needle_bytes = needle.as_bytes();
        if needle_bytes.is_empty() || needle_bytes.len() > bytes.len() {
            return false;
        }
        let mut start = 0usize;
        while let Some(pos) = lower[start..].find(needle) {
            let abs = start + pos;
            let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
            let end = abs + needle_bytes.len();
            let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
            start = abs + 1;
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locale_from_config() {
        assert_eq!(Locale::from_config(Some("en")), Locale::En);
        assert_eq!(Locale::from_config(Some("en-US")), Locale::En);
        assert_eq!(Locale::from_config(Some("zh-Hans")), Locale::Zh);
        assert_eq!(Locale::from_config(Some("zh")), Locale::Zh);
        assert_eq!(Locale::from_config(None), Locale::Zh);
    }

    #[test]
    fn test_new_session_message() {
        let msg = t(
            Msg::NewSessionStarted {
                topic_suffix: " (测试)",
            },
            Locale::Zh,
        );
        assert!(msg.contains("新对话已开始"));

        let msg = t(
            Msg::NewSessionStarted {
                topic_suffix: " (test)",
            },
            Locale::En,
        );
        assert!(msg.contains("New conversation started"));
    }

    #[test]
    fn render_loop_halt_hit_max_iterations_names_the_cap() {
        let msg = render_loop_halt(
            &TerminateReason::HitMaxIterations { used: 200 },
            200,
            42,
            Locale::En,
        );
        assert!(msg.contains("iteration cap"), "{msg}");
        assert!(msg.contains("200 iterations"), "{msg}");
        assert!(msg.contains("42 tool calls"), "{msg}");
        assert!(msg.contains("max_iterations"), "{msg}");
    }

    #[test]
    fn render_loop_halt_consecutive_failures_blames_tools_not_iterations() {
        let msg = render_loop_halt(
            &TerminateReason::ConsecutiveFailureCap { consecutive: 6 },
            6,
            6,
            Locale::Zh,
        );
        // Whole point of the fix: surface the real cause (tool failures /
        // network) and never point at the iteration cap, which is irrelevant.
        assert!(msg.contains("连续 6 轮工具调用全部失败"), "{msg}");
        assert!(msg.contains("工具配置或网络可达性"), "{msg}");
        assert!(
            !msg.contains("max_iterations"),
            "must NOT advise raising max_iterations for ConsecutiveFailureCap: {msg}"
        );
    }

    #[test]
    fn render_loop_halt_consecutive_failures_english_blames_tools_not_iterations() {
        let msg = render_loop_halt(
            &TerminateReason::ConsecutiveFailureCap { consecutive: 6 },
            6,
            6,
            Locale::En,
        );
        assert!(msg.contains("6 consecutive turns failed"), "{msg}");
        assert!(msg.contains("tool config or network reachability"), "{msg}");
        assert!(
            !msg.contains("max_iterations"),
            "must NOT advise raising max_iterations for ConsecutiveFailureCap: {msg}"
        );
    }

    #[test]
    fn render_loop_halt_stall_timeout_mentions_elapsed_ms() {
        let msg = render_loop_halt(
            &TerminateReason::StallTimeout { elapsed_ms: 30_000 },
            5,
            3,
            Locale::En,
        );
        assert!(msg.contains("30000 ms"), "{msg}");
        assert!(msg.contains("no progress"), "{msg}");
    }

    #[test]
    fn render_loop_halt_turn_timeout_names_phase() {
        let msg = render_loop_halt(
            &TerminateReason::TurnTimeout {
                phase: "act".into(),
                elapsed_ms: 12_000,
            },
            8,
            4,
            Locale::En,
        );
        assert!(msg.contains("`act`"), "{msg}");
        assert!(msg.contains("12000 ms"), "{msg}");
    }

    #[test]
    fn render_loop_halt_context_budget_advises_trim_session() {
        let msg = render_loop_halt(&TerminateReason::ContextBudgetExhausted, 50, 12, Locale::En);
        assert!(msg.contains("context budget"), "{msg}");
        assert!(msg.contains("trim") || msg.contains("new session"), "{msg}");
    }

    #[test]
    fn render_loop_halt_max_output_tokens_advises_token_knob() {
        let msg = render_loop_halt(&TerminateReason::MaxOutputTokensExhausted, 3, 0, Locale::En);
        assert!(msg.contains("max_output_tokens"), "{msg}");
        assert!(
            !msg.contains("max_iterations"),
            "max_output_tokens variant must not point at max_iterations: {msg}"
        );
    }

    #[test]
    fn render_loop_halt_stop_hook_surfaces_reason() {
        let msg = render_loop_halt(
            &TerminateReason::StopHookHalt {
                reason: "policy violation".into(),
            },
            2,
            0,
            Locale::En,
        );
        assert!(msg.contains("policy violation"), "{msg}");
    }

    #[test]
    fn render_loop_halt_completed_fallback_does_not_panic() {
        // Completed/Cancelled should never reach render_loop_halt (caller
        // gates on is_hit_limit) but if it does we must render a sensible
        // fallback instead of panicking.
        let msg = render_loop_halt(&TerminateReason::Completed, 10, 5, Locale::En);
        assert!(msg.contains("10 iterations"), "{msg}");
        assert!(msg.contains("5 tool calls"), "{msg}");
    }

    #[test]
    fn classify_buckets_common_failure_signatures() {
        assert_eq!(
            classify_error_text("429 Too Many Requests"),
            ReceiptKind::RateLimited
        );
        assert_eq!(
            classify_error_text("llm error: 401 Unauthorized"),
            ReceiptKind::Auth
        );
        assert_eq!(
            classify_error_text("connection refused"),
            ReceiptKind::Unreachable
        );
        assert_eq!(
            classify_error_text("request timed out"),
            ReceiptKind::Timeout
        );
        assert_eq!(
            classify_error_text("something nobody modelled"),
            ReceiptKind::Failed
        );
    }

    /// Regression: the engine's old `msg.contains("connection")` classifier
    /// read `disconnection_policy` as a network outage. Whole-token matching
    /// is the whole reason `contains_phrase` exists.
    #[test]
    fn classify_does_not_match_inside_longer_words() {
        assert_eq!(
            classify_error_text("disconnection_policy rejected the plan"),
            ReceiptKind::Failed
        );
        assert_eq!(
            classify_error_text("run 4291 aborted"),
            ReceiptKind::Failed,
            "a request id containing 429 must not read as rate limiting"
        );
    }

    /// Every bucket renders in both locales, and the wire codes stay stable.
    #[test]
    fn receipts_render_in_both_locales_with_stable_codes() {
        for kind in [
            ReceiptKind::Timeout,
            ReceiptKind::Cancelled,
            ReceiptKind::AgentBusy,
            ReceiptKind::RateLimited,
            ReceiptKind::Auth,
            ReceiptKind::Unreachable,
            ReceiptKind::Failed,
        ] {
            for locale in [Locale::Zh, Locale::En] {
                assert!(
                    !t(Msg::ErrReceipt(kind), locale).trim().is_empty(),
                    "{kind:?}/{locale:?} has no message"
                );
            }
            assert!(!kind.code().is_empty());
        }
        assert_eq!(ReceiptKind::Auth.code(), "AUTH");
        assert_eq!(
            ReceiptKind::Unreachable.code(),
            "PROVIDERS_UNREACHABLE",
            "wire code is client-visible API"
        );
        // The five below, plus the two above, are pinned literally rather
        // than derived: `protocol_code()`'s match is a bijection onto
        // `ReceiptCode`, so the set-based coverage test below
        // (`every_protocol_receipt_code_is_produced_by_some_kind`) cannot
        // catch two arms being SWAPPED — the image set is unchanged and
        // every other test in the repo still passes. A frozen wire contract
        // is exactly where a literal is correct; see `code()`'s own doc
        // ("these strings are API — do not rename").
        assert_eq!(ReceiptKind::Timeout.code(), "TIMEOUT");
        assert_eq!(ReceiptKind::Cancelled.code(), "CANCELLED");
        assert_eq!(ReceiptKind::AgentBusy.code(), "AGENT_BUSY");
        assert_eq!(ReceiptKind::RateLimited.code(), "RATE_LIMITED");
        assert_eq!(ReceiptKind::Failed.code(), "FAILED");
    }

    /// The wording is chosen by `Limit`'s shape, not by who is asking:
    /// `PerUser` renders both numbers (they are the caller's own),
    /// `Total` renders none — see `Limit::Total`'s doc for why a role
    /// predicate cannot substitute (there is no actor at render time to ask
    /// "may this caller see the machine total").
    #[test]
    fn spend_exhausted_receipt_renders_by_limit_shape() {
        let per_user = ReceiptKind::SpendExhausted {
            limit: Limit::PerUser {
                spent: 12.5,
                limit: 10.0,
            },
            reset_ms: 1_700_000_000_000,
        };
        let total = ReceiptKind::SpendExhausted {
            limit: Limit::Total,
            reset_ms: 1_700_000_000_000,
        };

        for kind in [per_user, total] {
            assert_eq!(kind.code(), "SPEND_EXHAUSTED");
            assert!(!kind.is_transient(), "a spend denial is never transient");
        }

        for locale in [Locale::Zh, Locale::En] {
            let per_user_msg = t(Msg::ErrReceipt(per_user), locale);
            assert!(
                per_user_msg.contains("12.5") && per_user_msg.contains("10.0"),
                "{locale:?} PerUser receipt must render both of the caller's own numbers: \
                 {per_user_msg}"
            );

            let total_msg = t(Msg::ErrReceipt(total), locale);
            assert!(
                !total_msg.contains('$'),
                "{locale:?} Total receipt must render no dollar figure: {total_msg}"
            );
        }
    }

    /// Every receipt names the reset instant, in both locales — the answer
    /// to "what do I do now" is on the card, not a second round trip away.
    #[test]
    fn spend_exhausted_receipt_names_the_reset_instant() {
        let reset_ms = 1_700_000_000_000_i64;
        let expected = chrono::DateTime::from_timestamp_millis(reset_ms)
            .unwrap()
            .to_rfc3339();
        for limit in [
            Limit::PerUser {
                spent: 5.0,
                limit: 5.0,
            },
            Limit::Total,
        ] {
            let kind = ReceiptKind::SpendExhausted { limit, reset_ms };
            for locale in [Locale::Zh, Locale::En] {
                let msg = t(Msg::ErrReceipt(kind), locale);
                assert!(
                    msg.contains(&expected),
                    "{limit:?}/{locale:?} must name the reset instant: {msg}"
                );
            }
        }
    }

    /// `is_transient()` — the single predicate `park_loop_on_transient_failure`
    /// and `block_goal_on_failure` both now call. Every bucket except the two
    /// provider-hiccup ones must read as terminal.
    #[test]
    fn is_transient_is_true_only_for_the_two_provider_hiccup_buckets() {
        let terminal = [
            ReceiptKind::Timeout,
            ReceiptKind::Cancelled,
            ReceiptKind::AgentBusy,
            ReceiptKind::Auth,
            ReceiptKind::Failed,
            ReceiptKind::SpendExhausted {
                limit: Limit::Total,
                reset_ms: 0,
            },
        ];
        for kind in terminal {
            assert!(!kind.is_transient(), "{kind:?} must not be transient");
        }
        assert!(ReceiptKind::RateLimited.is_transient());
        assert!(ReceiptKind::Unreachable.is_transient());
    }

    /// G12 — source-level: neither `Limit::Total` receipt arm (Zh or En) ever
    /// formats a dollar figure. Structurally near-impossible today — the
    /// `Limit::Total` pattern binds no numeric field, so `spent`/`limit`
    /// simply are not in scope inside that arm — but pinned as a source scan
    /// so a future refactor that pulls a number into scope (e.g. reads it
    /// off some other field in the outer error) cannot silently make the
    /// wrong figure formattable again. See `Limit::Total`'s own doc for why
    /// the machine-wide number must never reach this text.
    ///
    /// Scans only the production prefix (before `#[cfg(test)]`) with comment
    /// lines stripped, so this guard's own rule text — which necessarily
    /// contains the same needles it searches for — can never match itself
    /// and pass vacuously.
    #[test]
    fn no_limit_total_receipt_arm_formats_a_dollar_figure() {
        let full = include_str!("i18n.rs").replace('\r', "");
        let production = full
            .split("#[cfg(test)]")
            .next()
            .expect("this file always has a #[cfg(test)] boundary");
        let src: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !src.trim().is_empty(),
            "scan read nothing; the split point moved"
        );

        // Whitespace-insensitive by construction. These arm heads used to sit
        // on one line and this guard searched for that exact line; a routine
        // `cargo fmt` wrapped them across five lines and the literal silently
        // stopped matching. Reading tokens instead of layout means the next
        // reformat cannot blind it, and each body is bounded by the arm's own
        // closing brace rather than by a fixed indent or a line count.
        let flat: String = src.split_whitespace().collect::<Vec<_>>().join("");
        let bytes = flat.as_bytes();

        let mut checked = 0;
        for (locale_tag, locale_pat) in [("Zh", "Locale::Zh"), ("En", "Locale::En")] {
            let mut body = None;
            let mut cursor = 0usize;
            while let Some(rel) = flat[cursor..].find("ReceiptKind::SpendExhausted{") {
                let head_start = cursor + rel;
                let rel_arrow = flat[head_start..]
                    .find("=>{")
                    .expect("a match arm head always reaches its own `=> {`");
                let head = &flat[head_start..head_start + rel_arrow];
                cursor = head_start + rel_arrow + "=>{".len();
                // A real arm head is ~100 chars. Anything longer means this
                // occurrence was a construction site and the `=>{` we found
                // belongs to some later arm, so its text proves nothing.
                if head.len() > 200 || !head.contains("Limit::Total") || !head.contains(locale_pat)
                {
                    continue;
                }
                // Every brace in these bodies is balanced (`{}` placeholders
                // included), so depth returns to zero exactly at the arm's own
                // terminus — the unit's syntactic end, not a guessed window.
                let mut depth = 1usize;
                let mut i = cursor;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                assert_eq!(depth, 0, "{locale_tag} Limit::Total arm body never closes");
                // `i - 1` indexes the closing `}`; an ASCII byte can never be
                // the interior of a multi-byte char, so this stays on a
                // boundary even though the Zh body is not ASCII.
                body = Some(&flat[cursor..i - 1]);
                break;
            }
            let body =
                body.unwrap_or_else(|| panic!("{locale_tag} Limit::Total receipt arm not found"));
            assert!(
                !body.is_empty(),
                "{locale_tag} Limit::Total arm body scanned empty; the bound moved"
            );
            assert!(
                !body.contains('$'),
                "{locale_tag} Limit::Total arm must never render a dollar figure:\n{body}"
            );
            assert!(
                !body.contains("{spent") && !body.contains("{limit"),
                "{locale_tag} Limit::Total arm interpolated a spend number:\n{body}"
            );
            checked += 1;
        }
        assert_eq!(
            checked, 2,
            "expected exactly two Limit::Total arms (Zh + En)"
        );
    }

    /// An English-locale deployment must not receive Chinese error text — the
    /// exact regression the pre-unification `user_receipt` had (hardcoded zh).
    #[test]
    fn english_locale_receipts_are_english() {
        let msg = t(Msg::ErrReceipt(ReceiptKind::Auth), Locale::En);
        assert!(msg.contains("API key"), "{msg}");
        assert!(!msg.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)));
    }

    /// Every clarification-card key must actually be translated. A new entry
    /// whose Zh arm was pasted from the En arm is the failure mode this
    /// catches: it reads as "localized" everywhere else in the codebase and
    /// ships an English string to a Chinese user.
    #[test]
    fn every_clarification_key_is_translated_in_both_locales() {
        let keys = [
            Msg::ClarifyReplyFreeText,
            Msg::ClarifyReplyPickOne,
            Msg::ClarifyReplyPickMany,
            Msg::PlanApproveLabel,
            Msg::PlanReviseLabel,
            Msg::PlanRejectLabel,
            Msg::PlanObjectiveLead,
            Msg::PlanApprovePrompt,
        ];
        for key in keys {
            let en = t(key, Locale::En);
            let zh = t(key, Locale::Zh);
            assert!(!en.trim().is_empty(), "empty en arm");
            assert!(!zh.trim().is_empty(), "empty zh arm");
            assert_ne!(en, zh, "a key rendered identically in both locales: {en}");
            assert!(
                zh.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "the zh arm carries no Chinese: {zh}"
            );
            assert!(
                !en.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "the en arm carries Chinese: {en}"
            );
        }
    }

    /// An uninstalled process keeps emitting the bytes it emitted before the
    /// catalog existed. This is deliberately NOT `Locale::from_config`'s `Zh`
    /// default: these strings were English literals, and unit tests, CLI
    /// subcommands and embeddings never call `install_locale`.
    ///
    /// ⚠️ Reads a process-global, so it holds only while nothing in this test
    /// binary calls `install_locale` — today only the server binary does. A lib
    /// test that installs a locale would break this one and every English
    /// assertion in `clarification::render`; if one is ever needed, the global
    /// has to become a parameter first.
    #[test]
    fn the_ui_fallback_is_english_not_the_config_default() {
        assert_eq!(
            t_ui(Msg::ClarifyReplyPickOne),
            t(Msg::ClarifyReplyPickOne, Locale::En)
        );
        assert_eq!(Locale::from_config(None), Locale::Zh);
    }

    /// Server-side reconciliation: every protocol bucket must be reachable
    /// from some `ReceiptKind`, and every `ReceiptKind` must map to one. A
    /// literal list here would be the same enumeration bug one level up, so
    /// the expectation is derived from `ReceiptCode::ALL`.
    #[test]
    fn every_protocol_receipt_code_is_produced_by_some_kind() {
        use aleph_protocol::receipt::ReceiptCode;
        use std::collections::HashSet;

        let produced: HashSet<ReceiptCode> = [
            ReceiptKind::Timeout,
            ReceiptKind::Cancelled,
            ReceiptKind::AgentBusy,
            ReceiptKind::RateLimited,
            ReceiptKind::Auth,
            ReceiptKind::Unreachable,
            ReceiptKind::Failed,
            ReceiptKind::SpendExhausted {
                limit: Limit::Total,
                reset_ms: 0,
            },
        ]
        .into_iter()
        .map(ReceiptKind::protocol_code)
        .collect();

        let expected: HashSet<ReceiptCode> = ReceiptCode::ALL.iter().copied().collect();
        assert_eq!(
            produced, expected,
            "a protocol bucket with no producing ReceiptKind is unreachable; \
             a ReceiptKind with no bucket cannot be rendered by any client"
        );
    }
}
