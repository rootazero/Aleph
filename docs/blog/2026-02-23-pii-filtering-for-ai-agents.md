# 给 AI Agent 加一道 PII 过滤网关：从 1228 条误报到 58 条的调优实战

你跟 AI 聊天时说的每一句话，都去了哪？

答案很简单——云端 API。不管你用的是 ChatGPT、Claude 还是自建的 Agent 系统，只要模型不是本地跑的，你的对话内容就在互联网上裸奔。

这本身不是新闻。但当 AI Agent 接入了真实生活——处理邮件、分析文档、查快递——问题就变了。你不再是在说"帮我写首诗"，而是"帮我查一下 138xxxx1234 这个号码的快递"、"把这份合同里的身份证号提取出来"。

手机号、邮箱、身份证、银行卡号——这些东西混在对话里，安安静静地坐上了发往 API provider 的 HTTPS 请求。

我扫了一遍自己 Agent 系统的历史 session 日志，结果吓了一跳——1500 多个 session、上万条消息里，到处散落着真实的个人信息。

于是我给系统加了一道 PII 过滤网关。今天聊聊怎么做的，踩了哪些坑。

---

## 你到底在保护什么

PII（Personal Identifiable Information），个人身份信息。在中国语境下，核心就这几类：

| 类型 | 特征 | 敏感度 |
|------|------|--------|
| 身份证号 | 18 位，最后一位可能是 X | Critical |
| 银行卡号 | 16-19 位数字，有 Luhn 校验 | High |
| 手机号 | 11 位，1 开头，第二位 3-9 | High |
| API Key / Token | sk-、ghp\_、AKIA 等前缀 | Critical |
| SSH 私钥 | -----BEGIN 开头的 PEM 块 | Critical |
| 邮箱地址 | 不用解释 | Medium |
| IP 地址 | 内网外网都算 | Low |

7 类规则，覆盖了 AI Agent 系统里最常见的泄露场景。

## 为什么不用 Presidio

你可能会问：微软的 Presidio、Google 的 DLP API 不香吗？

对于一个本地跑的 Agent 系统来说，Presidio 太重了。ML-based 的 NER 方案要装一堆依赖，模型加载要时间，还得联网下载模型文件。而且 NER 模型擅长的是从自然语言里识别"这是人名""这是地址"——但手机号、身份证这些东西，**格式是固定的，正则表达式比 NER 精准十倍**。

所以我选了最简单的方案：**纯正则 + 规则引擎**。零依赖，可解释，本地运行，够用。

---

## 写正则谁都会，难的是不误报

规则引擎写起来不难。手机号 `1[3-9]\d{9}`，邮箱一个标准正则，身份证 18 位数字加结构校验——半天就能写完。

然后你跑一遍测试，发现命中了 1228 条"PII"。

你兴冲冲打开一看——80% 是误报。

**误报调优才是这个项目的核心战役。** 从 1228 降到 58，95% 的降幅，花的时间比写规则多了十倍。

这里有一个关键的设计差异：**日志脱敏和网关过滤的要求完全相反**。日志脱敏宁可误杀也不放过（false positives acceptable），因为日志里少几个数字无所谓。但网关过滤恰恰相反——误报会破坏发给 LLM 的消息内容，直接影响 AI 的理解能力。**误报率比漏报率重要。**

让我细数这些坑。

### 坑 1：Discord Snowflake ID 变成身份证

Discord 的消息 ID、用户 ID、频道 ID 都是 Snowflake 格式——17 到 20 位的纯数字：

```
1468256454954975286
```

18 位，正好撞上身份证的长度。某些 Snowflake 甚至能通过 Luhn 校验，被当成银行卡。Agent 系统天天跟 Discord 打交道，日志里全是这种 ID，不处理的话每条消息都会被标红。

**解法：身份证必须做真正的结构验证。** 不是随便 18 位数字就是身份证。中国身份证有严格的结构：

```rust
// 区域码验证：前 2 位必须在 11-82 范围内
let region: u32 = digits[0..2].parse()?;
if region < 11 || region > 82 { return false; }

// 日期验证：第 7-14 位必须是合法的 YYYYMMDD
let year: u32 = digits[6..10].parse()?;
let month: u32 = digits[10..12].parse()?;
let day: u32 = digits[12..14].parse()?;
if year < 1900 || year > 2100 { return false; }
if month < 1 || month > 12 { return false; }
if day < 1 || day > 31 { return false; }

// 校验码：ISO 7064 MOD 11-2 算法
let weights = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
let check_codes = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
let sum: u32 = digits[0..17].iter()
    .zip(weights.iter())
    .map(|(d, w)| d * w)
    .sum();
let expected = check_codes[(sum % 11) as usize];
```

加了这三层验证后，Snowflake ID 和随机数字串被 100% 排除。

### 坑 2：UUID 片段变成手机号

这是最骚的一个。UUID 长这样：

```
550e8400-e29b-41d4-a716-446655440000
```

但在某些日志格式里，UUID 会被截断或者拼接到其他字符串里：

```
18160019229f-4b7a-8c3d-...
```

正则引擎看到 `18160019229`——11 位，1 开头，第二位是 8——完美匹配手机号模式。

**解法：加 hex 边界检测。** 如果匹配到的"手机号"前后紧跟着十六进制字符（a-f），那大概率是 UUID 的一部分，跳过。

```rust
fn is_hex_bounded(text: &str, start: usize, end: usize) -> bool {
    if start > 0 {
        if let Some(c) = text[..start].chars().last() {
            // a-f (not 0-9) before the match = likely UUID
            if c.is_ascii_hexdigit() && !c.is_ascii_digit() {
                return true;
            }
        }
    }
    if end < text.len() {
        if let Some(c) = text[end..].chars().next() {
            if c.is_ascii_hexdigit() && !c.is_ascii_digit() {
                return true;
            }
        }
    }
    false
}
```

这个规则一加，误报直接砍掉一大片。

### 坑 3：Unix 时间戳伪装成手机号

```
1389168000  — 2014-01-08 的 Unix 时间戳
```

13 开头，如果日志里时间戳后面跟了个别的数字——`13891680001`——就成了完美的手机号。

**解法：上下文检查。** 扫描匹配位置前后 40 个字符，如果出现 `timestamp`、`created_at`、`updated_at` 等关键词，降低置信度，跳过。

```rust
fn is_timestamp_context(text: &str, start: usize) -> bool {
    // 注意 UTF-8 边界！中文字符是多字节的
    let mut ctx_start = start.saturating_sub(40);
    while ctx_start > 0 && !text.is_char_boundary(ctx_start) {
        ctx_start -= 1;
    }
    let mut ctx_end = (start + 40).min(text.len());
    while ctx_end < text.len() && !text.is_char_boundary(ctx_end) {
        ctx_end += 1;
    }
    let context = &text[ctx_start..ctx_end];
    TIMESTAMP_KEYWORDS.is_match(context)
}
```

这里有个隐蔽的坑：**UTF-8 字符边界**。如果你的 Agent 系统处理中文消息（几乎肯定会），`start - 40` 可能落在一个中文字符的中间，直接 panic。必须用 `is_char_boundary()` 做安全偏移。这个 bug 我是在代码审查时才发现的。

### 坑 4：JSON 浮点数变成银行卡

JSON 日志里经常有这种东西：

```json
{"amount": 6222021234567890.50}
```

`6222021234567890`——16 位，622 开头——银行卡号的经典格式。

**解法：检测数字前后是否有小数点。** 有小数点的跳过，那是浮点数不是卡号。同时银行卡还要过 **Luhn 算法**校验：

```rust
fn luhn_check(digits: &str) -> bool {
    let sum: u32 = digits.chars().rev().enumerate().map(|(i, c)| {
        let mut d = c.to_digit(10).unwrap();
        if i % 2 == 1 {
            d *= 2;
            if d > 9 { d -= 9; }
        }
        d
    }).sum();
    sum % 10 == 0
}
```

### 坑 5：URL slug 变成 API Key

很多 API Key 的特征是"一长串字母数字混合"。但 URL 路径里也经常有这种东西：

```
/api/v1/sessions/a7b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6
```

正则看到一长串随机字符，开心地标记为"疑似 API Key"。

**解法：只做前缀匹配。** 只有以 `sk-`、`ghp_`、`AKIA`、`xox`、`tvly-` 等已知前缀开头的才标记。通用的长字符串不管——宁可漏过，不能误报。

---

## 架构决策：在哪里拦截

PII 过滤可以放在两个位置：

**方案 A：API 网关层。** 在 Agent 和 API provider 之间加一个代理，所有请求经过时扫描和脱敏。一劳永逸，但增加延迟，需要解析请求体。

**方案 B：Provider 层注入。** 在 HTTP 请求发出前的最后一步做检查。

我选了方案 B——具体来说，在所有 LLM HTTP 请求的统一出口注入过滤：

```
用户消息 → Agent Loop → Thinker → Provider.execute()
                                        ↓
                              PiiEngine.filter(payload.input)
                                        ↓
                              ProtocolAdapter.build_request()
                                        ↓
                                   HTTP → API
```

这个位置的好处：

1. **最接近出口** = 最安全，不可能遗漏
2. **改动集中** = 只改一个方法，所有协议（OpenAI、Anthropic、Gemini）自动覆盖
3. **不污染核心** = Agent Loop 和 Thinker 完全不知道过滤的存在
4. **System prompt 不过滤** = 系统提示词由 Agent 生成，不含用户 PII

关键实现：

```rust
async fn execute(&self, payload: RequestPayload<'_>) -> Result<String> {
    // PII 过滤：发 API 前的最后一道防线
    let filtered_input;
    let final_payload = if let Some(engine) = PiiEngine::global() {
        if let Ok(guard) = engine.read() {
            if !guard.is_provider_excluded(&self.name) {
                let result = guard.filter(payload.input);
                if result.has_detections() {
                    filtered_input = result.text;
                    RequestPayload { input: &filtered_input, ..payload }
                } else { payload }
            } else { payload }  // Ollama 等本地模型跳过
        } else { payload }
    } else { payload };  // 引擎未初始化，放行

    self.adapter.build_request(&final_payload, &self.config, false)?
        .send().await?
}
```

注意几个设计选择：

- **全局单例 + RwLock**：引擎在服务启动时初始化一次，多线程共享读锁，接近零开销
- **Provider 白名单**：本地 Ollama 不需要过滤，配置 `exclude_providers = ["ollama"]` 跳过
- **优雅降级**：引擎未初始化时直接放行，不阻塞正常流程

---

## 配置驱动，分级处理

不是所有 PII 都一样敏感。身份证号绝对不能泄露，邮箱地址可能是公开的。所以我做了**分级处理**：

```toml
[privacy]
pii_filtering = true

# 三种模式：
# "block" = 替换为占位符（[PHONE]、[ID_CARD] 等）
# "warn"  = 记录日志但不替换
# "off"   = 完全忽略

id_card = "block"         # Critical: 绝不泄露
bank_card = "block"       # High
phone = "block"           # High
api_key = "block"         # Critical
ssh_key = "block"         # Critical
email = "warn"            # Medium: 记录但不阻断
ip_address = "off"        # Low: 不检测

exclude_providers = ["ollama"]
```

而且支持**热重载**——改配置文件后，服务自动更新策略，不用重启。

---

## 白名单：对付测试数据

你的系统里一定有测试号码、示例邮箱、mock 数据。这些不是泄露，但会严重干扰检测结果。

```rust
// 测试号码
test_phones: ["13800138000", "18888888888", "13900001111"]

// 系统邮箱
system_emails: [r"noreply@.*", r".*@example\.com", r".*@(test|demo|sample)\..*"]

// 本地 IP
local_ips: ["127.0.0.1", "0.0.0.0", "192.168.1.1", "10.0.0.1"]
```

没有白名单的话，测试号码 `13800138000` 会在每次单元测试里触发告警——然后你会因为烦而关掉这个功能——然后所有真正的 PII 都没人管了。

---

## 审计日志：只记元数据

过滤发生时，记录审计日志但**绝不记录原始 PII 值**：

```rust
tracing::warn!(
    rule = "phone",
    severity = "high",
    "PII detected and blocked before API call"
);
```

日志里只有规则名和严重级别。如果你把匹配到的手机号也写进日志——恭喜，你刚刚用日志系统泄露了你试图保护的隐私。

---

## 数字

最终结果：

- **7 类规则**，106 个测试，0 失败
- **6 项反误报机制**：hex 边界、时间戳上下文、ISO 7064 校验码、Luhn 算法、小数点排除、前缀匹配
- 过滤延迟 < 1ms（正则预编译，OnceLock 全局单例）
- 从第一版的 1228 条命中降到 58 条真实 PII——**95.3% 的误报被消除**

---

## 最常见的泄露场景

审计历史 session 后，我发现泄露几乎是不经意间发生的：

1. **用户主动提供**："帮我查一下这个手机号"——你说的每个字都会发给 API
2. **文档处理**：Agent 读取包含个人信息的文件，内容进入 context window
3. **系统日志**：错误日志里带了用户信息，被 Agent 读取后发送
4. **工具调用结果**：Agent 调用外部 API，返回结果里包含个人信息

每一个场景都很自然。没有人刻意泄露，但隐私就是这么流出去的。

---

## 为什么不用 ML：一个务实的选择

正则够用的场景，不要上 ML。

手机号、身份证、银行卡——格式是确定性的。正则可以做到 100% 的召回率（格式对就一定能匹配），误报率通过规则调优可以压到很低。ML 模型在这些场景下不会更好，反而引入不确定性。

正则方案有几个 ML 方案比不了的优势：

| | 正则 | ML/NER |
|---|---|---|
| 依赖 | 零 | 模型文件 + 推理库 |
| 可解释性 | "手机号格式匹配" | "置信度 0.87" |
| 运行环境 | 任何地方 | 需要 GPU 或至少算力 |
| 确定性 | 同输入永远同输出 | 模型版本升级可能变化 |
| 启动时间 | 毫秒级 | 秒级（加载模型） |

ML 真正有优势的是非结构化 PII——人名、地址、公司名。但 80/20 法则：结构化 PII 占泄露的绝大多数。先用正则搞定这 80%，剩下的以后再说。

---

## 实操建议

如果你也想给 Agent 系统加一道 PII 网关：

**先审计，再拦截。** 不要上来就做实时拦截。先扫一遍历史数据，搞清楚你的系统里到底有哪些 PII。没有基线，你不知道自己在保护什么。

**误报率比漏报率重要。** 漏掉一个手机号，风险有限。但误报太多，你会关掉这个功能。

**日志脱敏和网关过滤用不同的引擎。** 日志脱敏可以宽松（宁可误杀），网关过滤必须精确（不能破坏 LLM 输入）。我的系统里这两套引擎并存。

**测试数据是大坑。** 提前建好白名单，排除测试号码、示例邮箱、本地域名。

**上下文很关键。** 同一个 11 位数字，在"给 138xxxx1234 发短信"里是手机号，在 `timestamp` 字段里是时间戳。简单的启发式规则（检查周围的 key 名称）就能过滤掉大部分误报。

**注意 UTF-8。** 如果你的系统处理中文，字符串切片必须做边界检查。这是最容易遗漏的 runtime panic 来源。

---

## 演进路径

```
阶段 1（现在）：正则引擎覆盖结构化 PII + 反误报调优 + 白名单
      ↓
阶段 2：扫描历史 session → 发现 PII → 写入保护名单 → 自动拦截
      ↓
阶段 3：本地 NER 模型（GLiNER / 小型 BERT）处理人名、地址
```

阶段 1 已经能解决 90% 的问题。剩下的 10%，等真正需要的时候再做也不迟。

---

## 最后

你现在用的 AI 工具——ChatGPT、Claude、各种 Agent 框架——你跟它们说过的话里，有多少个人信息？

这些信息现在躺在某个云端服务器的日志里。也许它们承诺不会用于训练，也许它们有完善的数据保护政策。但承诺和现实之间，永远隔着一道你看不见的墙。

如果你自己搭了 Agent 系统，PII 过滤不是可选项——是必选项。

不难做，正则就够。难的是把误报调到能用的水平。

但这个投入是值得的。毕竟，你的身份证号泄露一次就相当于永久公开了。
