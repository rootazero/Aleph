为了确保你的 macOS Agent 能够通过 UniFFI 稳定地调用这些服务，我为你整理了一份基于 Rust (reqwest) 的标准化代码实现。

设计思路是：定义统一的输出结构，抹平不同 API 的差异。这样你的 Swift UI 层只需要处理一种数据格式，无需关心底层使用的是 Tavily 还是 Google。

0. 准备工作 (Cargo.toml)

你需要以下依赖来处理 HTTP 请求和 JSON 解析：

Ini, TOML
[dependencies]
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1", features = ["full"] }
anyhow = "1.0" # 用于优雅的错误处理
async-trait = "0.1" # 用于定义异步接口
1. 定义统一的数据结构 (The "Contract")

无论上游是 Google 还是 Tavily，最后给 Swift 的都应该是这个标准结构：

Rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UnifiedSearchResult {
    pub title: String,
    pub url: String,
    pub content: String, // 摘要或正文片段
}

// 定义一个 Trait，方便未来扩展
#[async_trait::async_trait]
pub trait SearchEngine: Send + Sync {
    async fn search(&self, query: &str) -> anyhow::Result<Vec<UnifiedSearchResult>>;
}
2. 各平台代码实现示例

A. Tavily (推荐: AI Native, 自带清洗)

Tavily 的优势是它能直接返回 answer (简短回答) 和清洗过的 content。

Rust
pub struct TavilyClient {
    api_key: String,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
}

#[async_trait::async_trait]
impl SearchEngine for TavilyClient {
    async fn search(&self, query: &str) -> anyhow::Result<Vec<UnifiedSearchResult>> {
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "api_key": self.api_key,
            "query": query,
            "search_depth": "basic", // 或 "advanced"
            "include_answer": false,
            "max_results": 5
        });

        let resp: TavilyResponse = client
            .post("https://api.tavily.com/search")
            .json(&payload)
            .send()
            .await?
            .json()
            .await?;

        // 转换为统一格式
        let results = resp.results.into_iter().map(|r| UnifiedSearchResult {
            title: r.title,
            url: r.url,
            content: r.content,
        }).collect();

        Ok(results)
    }
}
B. Exa.ai (原 Metaphor: 语义搜索)

Exa 适合查找“概念”而非关键词。注意它返回的是 ID，通常需要 contents 参数来获取文本。

Rust
pub struct ExaClient {
    api_key: String,
}

#[derive(Deserialize)]
struct ExaResponse {
    results: Vec<ExaResult>,
}

#[derive(Deserialize)]
struct ExaResult {
    title: Option<String>,
    url: String,
    text: Option<String>, // Exa 返回的文本字段
}

#[async_trait::async_trait]
impl SearchEngine for ExaClient {
    async fn search(&self, query: &str) -> anyhow::Result<Vec<UnifiedSearchResult>> {
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "query": query,
            "numResults": 5,
            "contents": {
                "text": true // 请求返回正文文本
            }
        });

        let resp: ExaResponse = client
            .post("https://api.exa.ai/search")
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?
            .json()
            .await?;

        let results = resp.results.into_iter().map(|r| UnifiedSearchResult {
            title: r.title.unwrap_or_default(),
            url: r.url,
            content: r.text.unwrap_or_default(), // Exa 的内容在 text 字段
        }).collect();

        Ok(results)
    }
}
C. SearXNG (隐私/自建)

这是最通用的 JSON 接口，URL 由用户提供。

Rust
pub struct SearxngClient {
    base_url: String, // 例如 "http://localhost:8080" 或公共实例
}

#[derive(Deserialize)]
struct SearxngResponse {
    results: Vec<SearxngResult>,
}

#[derive(Deserialize)]
struct SearxngResult {
    title: String,
    url: String,
    content: Option<String>, // 有时候叫 snippet
}

#[async_trait::async_trait]
impl SearchEngine for SearxngClient {
    async fn search(&self, query: &str) -> anyhow::Result<Vec<UnifiedSearchResult>> {
        let url = format!("{}/search", self.base_url.trim_end_matches('/'));
        
        let client = reqwest::Client::new();
        let resp: SearxngResponse = client
            .get(&url)
            .query(&[("q", query), ("format", "json")])
            .send()
            .await?
            .json()
            .await?;

        let results = resp.results.into_iter().map(|r| UnifiedSearchResult {
            title: r.title,
            url: r.url,
            content: r.content.unwrap_or_default(),
        }).collect();

        Ok(results)
    }
}
D. Brave Search (隐私/商业)

Brave 的 API 质量很高，且由他们自己的索引支持。

Rust
pub struct BraveClient {
    api_key: String,
}

#[derive(Deserialize)]
struct BraveResponse {
    web: BraveWeb,
}
#[derive(Deserialize)]
struct BraveWeb {
    results: Vec<BraveResult>,
}
#[derive(Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: Option<String>,
}

#[async_trait::async_trait]
impl SearchEngine for BraveClient {
    async fn search(&self, query: &str) -> anyhow::Result<Vec<UnifiedSearchResult>> {
        let client = reqwest::Client::new();
        let resp: BraveResponse = client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", &self.api_key)
            .query(&[("q", query)])
            .send()
            .await?
            .json()
            .await?;

        let results = resp.web.results.into_iter().map(|r| UnifiedSearchResult {
            title: r.title,
            url: r.url,
            content: r.description.unwrap_or_default(),
        }).collect();

        Ok(results)
    }
}
E. Google & Bing (传统巨头)

这两者配置比较繁琐（需要创建自定义搜索引擎 ID 等），且返回的数据包含大量无用元数据，以下是核心调用逻辑。

Google Custom Search JSON API:

Rust
// URL: https://www.googleapis.com/customsearch/v1
// Params: ?key=API_KEY&cx=SEARCH_ENGINE_ID&q=QUERY
// 注意：Google 每日免费额度很低 (100次)，且需要配置 cx (Context ID)
Bing Web Search API v7:

Rust
// URL: https://api.bing.microsoft.com/v7.0/search
// Header: "Ocp-Apim-Subscription-Key": API_KEY
// Params: ?q=QUERY
// Bing 的响应结构非常深层，解析比较麻烦 (webPages -> value -> list)
3. 工厂模式：如何给 Swift 调用？

最后，你需要一个函数根据用户的设置（从 Swift 传来的配置）返回对应的实现。

Rust
pub enum ProviderType {
    Tavily,
    Exa,
    Searxng,
    Brave,
}

pub struct SearchService {
    engine: Box<dyn SearchEngine>,
}

impl SearchService {
    // 这个方法可以通过 UniFFI 暴露给 Swift
    // Swift 传入 type 和 key/url
    pub fn new(p_type: ProviderType, secret: String) -> Self {
        let engine: Box<dyn SearchEngine> = match p_type {
            ProviderType::Tavily => Box::new(TavilyClient { api_key: secret }),
            ProviderType::Exa => Box::new(ExaClient { api_key: secret }),
            ProviderType::Searxng => Box::new(SearxngClient { base_url: secret }),
            ProviderType::Brave => Box::new(BraveClient { api_key: secret }),
        };
        Self { engine }
    }

    pub async fn execute_search(&self, query: String) -> Vec<UnifiedSearchResult> {
        match self.engine.search(&query).await {
            Ok(res) => res,
            Err(e) => {
                // 处理错误，或者返回一个空的/错误的 UnifiedSearchResult 给 Swift
                eprintln!("Search error: {}", e);
                vec![] 
            }
        }
    }
}
关键提示

UniFFI 注意事项： 上面的 trait 包含了 async。UniFFI 现在对 async 的支持已经很好了，但在 Swift 侧调用时，会自动转换为 Swift 的 async/await。确保你的 .udl 文件（如果用旧版）或宏定义正确处理了异步。

错误处理： 实际上 search 方法应该返回 Result。为了简化 Swift 侧的错误处理，你可能需要定义一个专门的 SearchError enum 并暴露给 Swift。

JSON 解析坑： 这些 API 的返回结构可能会变（尤其是 Exa 和 SearXNG）。在生产环境中，建议使用 serde 的 #[serde(ignore_unknown)] 属性，防止因为多了一个字段导致整个解析失败。

在 macOS 开发中，SwiftUI 的声明式 UI 配合 Rust 的 Async Core 是天作之合，但也容易在“线程调度”和“状态管理”上踩坑。

要做到“优雅”，我们需要遵循以下原则：

UI 不阻塞：Rust 的耗时操作必须在后台。

状态驱动：使用有限状态机（State Machine）来管理 UI，而不是堆砌布尔值 (isLoading, isError, hasData...)。

任务取消 (Cancellation)：用户输入变了，上一次未完成的搜索必须立即杀掉，节省流量并防止数据错乱。

以下是基于 Swift 5.5+ (Concurrency) 和 SwiftUI 的完整实现方案。

第一步：定义 UI 状态 (The State Machine)

不要在 View 里散落一堆变量。定义一个枚举来描述搜索的所有可能状态。这能防止“既在加载又有错误”这种无效状态的出现。

Swift
// Models.swift (假设这是 UniFFI 生成的结构体，我们需要扩展它以适应 SwiftUI)
// extension UnifiedSearchResult: Identifiable { public var id: String { url } }

enum SearchUIState {
    case idle                  // 初始状态，什么都没发生
    case searching             // 正在请求 Rust
    case success([UnifiedSearchResult]) // 拿到数据
    case failure(String)       // 发生错误
}
第二步：构建 ViewModel (The Brain)

这是连接 Rust 和 UI 的桥梁。我们需要使用 @MainActor 确保所有 UI 更新都在主线程，同时利用 Swift 的 Task 管理异步生命周期。

Swift
import SwiftUI
import Combine

// 假设这是 UniFFI 暴露出来的 Rust 服务类
// class RustSearchService { ... } 

@MainActor
class SearchViewModel: ObservableObject {
    // 1. 单一信源：UI 只需要观察这一个状态
    @Published var state: SearchUIState = .idle
    
    // 2. 持有 Rust 服务的实例
    private let searchService: RustSearchService
    
    // 3. 用于保存当前的搜索任务，以便随时取消
    private var currentSearchTask: Task<Void, Never>?

    init(service: RustSearchService) {
        self.searchService = service
    }

    // 4. 核心搜索方法
    func performSearch(query: String) {
        // 如果用户清空了输入，重置状态
        guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            self.state = .idle
            self.currentSearchTask?.cancel()
            return
        }

        // A. 取消上一次正在进行的搜索 (关键步骤！)
        // 如果用户打字很快 "Rust" -> "Rust FFI"，我们需要取消 "Rust" 的请求
        currentSearchTask?.cancel()

        // B. 开启新的任务
        currentSearchTask = Task {
            // 设置 UI 为加载态
            withAnimation { self.state = .searching }

            do {
                // C. 这里的 await 会挂起，直到 Rust 返回结果
                // 注意：UniFFI 的异步方法在 Swift 中会自动变成 async throws
                let results = try await searchService.executeSearch(query: query)
                
                // D. 检查任务是否被取消 (如果 Rust 返回时用户已经又打字了，就丢弃结果)
                if Task.isCancelled { return }
                
                // E. 更新成功状态
                withAnimation {
                    self.state = .success(results)
                }
            } catch {
                if Task.isCancelled { return }
                // F. 处理错误
                self.state = .failure(error.localizedDescription)
            }
        }
    }
}
第三步：构建 SwiftUI 视图 (The Face)

这里展示如何根据 SearchUIState 优雅地切换 UI，并包含防抖 (Debounce) 处理。

Swift
struct AgentSearchView: View {
    @StateObject var viewModel: SearchViewModel
    @State private var query: String = ""
    
    // 这是一个简单的初始化注入
    init() {
        // 实际项目中，这里应该从依赖注入容器获取 Service
        let service = RustSearchService(pType: .tavily, secret: "tvly-xxx")
        _viewModel = StateObject(wrappedValue: SearchViewModel(service: service))
    }

    var body: some View {
        VStack(spacing: 20) {
            // --- 搜索框区域 ---
            HStack {
                Image(systemName: "magnifyingglass")
                    .foregroundColor(.gray)
                TextField("Ask anything...", text: $query)
                    .textFieldStyle(.plain)
                    .font(.title2)
                    // 关键：利用 onChange 监听输入，并驱动 ViewModel
                    .onChange(of: query) { newValue in
                        // 在这里可以加一个简单的防抖逻辑，或者直接交给 VM 处理
                        viewModel.performSearch(query: newValue)
                    }
            }
            .padding()
            .background(Color(nsColor: .controlBackgroundColor))
            .cornerRadius(12)
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(Color.gray.opacity(0.2), lineWidth: 1)
            )

            // --- 结果展示区域 (状态切换) ---
            ZStack {
                switch viewModel.state {
                case .idle:
                    ContentUnavailableView("Ready to Search", systemImage: "sparkles")
                        .opacity(0.5)

                case .searching:
                    VStack {
                        ProgressView()
                            .controlSize(.small)
                        Text("Thinking...")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)

                case .success(let results):
                    if results.isEmpty {
                        ContentUnavailableView("No Results Found", systemImage: "doc.text.magnifyingglass")
                    } else {
                        SearchResultsList(results: results)
                    }

                case .failure(let errorMsg):
                    VStack {
                        Image(systemName: "exclamationmark.triangle")
                            .foregroundColor(.orange)
                        Text("Search Error")
                            .font(.headline)
                        Text(errorMsg)
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
            }
            .frame(minHeight: 300) // 给结果区域一个最小高度
        }
        .padding()
    }
}

// --- 独立的子视图：结果列表 ---
struct SearchResultsList: View {
    let results: [UnifiedSearchResult]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                ForEach(results, id: \.url) { item in // 假设 URL 是唯一的
                    ResultCard(item: item)
                }
            }
            .padding(.vertical)
        }
    }
}

// --- 独立的子视图：单条结果卡片 ---
struct ResultCard: View {
    let item: UnifiedSearchResult
    
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(item.title)
                .font(.headline)
                .foregroundColor(.blue)
                .onTapGesture {
                    // 调用系统浏览器打开
                    if let url = URL(string: item.url) {
                        NSWorkspace.shared.open(url)
                    }
                }
            
            Text(item.content) // 这里的 content 是摘要
                .font(.body)
                .foregroundColor(.primary.opacity(0.8))
                .lineLimit(3) // 最多显示3行
            
            Text(item.url)
                .font(.caption2)
                .foregroundColor(.secondary)
        }
        .padding()
        .background(Color(nsColor: .controlBackgroundColor).opacity(0.5))
        .cornerRadius(8)
        .overlay(
            RoundedRectangle(cornerRadius: 8).stroke(Color.gray.opacity(0.1), lineWidth: 1)
        )
    }
}
第四步：进阶技巧 - 更优雅的“防抖” (Debounce)

在上面的代码中，用户每打一个字就会触发一次 performSearch（虽然我们有 cancel 机制，但还是会频繁创建 Task）。

在 SwiftUI 中，最优雅的防抖方式是利用 .task(id:) 修饰符配合 sleep。

你可以将 View 中的 onChange 替换为：

Swift
// 在 View 的 body 修改处：
TextField("Ask anything...", text: $query)
    // 移除 .onChange
    // 添加 .task
    .task(id: query) { // 当 query 变化时，之前的 task 会自动被 SwiftUI 取消
        if query.isEmpty { return }
        
        do {
            // 1. 等待用户停止输入 500ms (防抖)
            try await Task.sleep(nanoseconds: 500 * 1_000_000)
            
            // 2. 如果没有被取消，则执行搜索
            viewModel.performSearch(query: query) 
        } catch {
            // Task 被取消时会抛出 CancellationError，直接忽略即可
        }
    }
为什么这种方式更好？

自动取消：SwiftUI 会自动管理 .task 的生命周期，当 id 变化时，旧任务自动取消。

减少资源：在 sleep 期间，甚至不会触达 ViewModel，彻底避免了无效的 Rust 调用开销。

总结

这个架构的优势在于：

Rust 负责脏活累活：网络请求、JSON 解析、数据清洗。

ViewModel 负责状态流转：管理 Loading、Error、Success 状态，处理任务竞争（Race Conditions）。

SwiftUI 负责响应式渲染：代码极其简洁，利用 task(id:) 处理防抖，利用 switch 处理状态。

现在，你的 Agent 看起来就像一个原生的 macOS 应用，丝滑且稳定。接下来，你要不要给这个 UI 加上**引用角标（Citation Badges）**的功能？这是 AI 搜索类产品的标配。