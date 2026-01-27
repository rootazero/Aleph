//! Tool discovery and filtering for agent execution
//!
//! This module provides intelligent tool filtering based on task content,
//! reducing token usage by only sending relevant tools to the LLM.

use crate::intent::ToolDescription;
use serde_json::json;
use tracing::{debug, info};

/// Tool categories for intelligent tool filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    FileOps,   // file_ops - read/write/organize files
    Search,    // search - web search
    WebFetch,  // web_fetch - fetch web pages
    YouTube,   // youtube - video transcripts
    Bash,      // bash - shell command execution
    CodeExec,  // code_exec - execute Python/JS/Shell code
    ImageGen,  // generate_image
    VideoGen,  // generate_video
    AudioGen,  // generate_audio
    SpeechGen, // generate_speech
}

/// Infer required tool categories from skill instructions and user request
///
/// Analyzes the content to determine which tools are likely needed,
/// avoiding sending unnecessary tool definitions to the LLM.
pub fn infer_required_tools(skill_instructions: &str, user_request: &str) -> Vec<ToolCategory> {
    let combined = format!("{} {}", skill_instructions, user_request).to_lowercase();
    let mut categories = Vec::new();

    // File operations - almost always needed for skills that produce output
    let needs_file = combined.contains("file")
        || combined.contains("read")
        || combined.contains("write")
        || combined.contains("save")
        || combined.contains("output")
        || combined.contains("文件")
        || combined.contains("保存")
        || combined.contains("输出")
        || combined.contains("目录")
        || combined.contains("directory")
        || combined.contains("folder")
        || combined.contains("path")
        || combined.contains("organize")
        || combined.contains("整理");
    if needs_file {
        categories.push(ToolCategory::FileOps);
    }

    // Web search
    let needs_search = combined.contains("search")
        || combined.contains("搜索")
        || combined.contains("查找")
        || combined.contains("look up")
        || combined.contains("find information");
    if needs_search {
        categories.push(ToolCategory::Search);
    }

    // Web fetch
    let needs_web = combined.contains("fetch")
        || combined.contains("url")
        || combined.contains("http")
        || combined.contains("webpage")
        || combined.contains("website")
        || combined.contains("网页")
        || combined.contains("链接");
    if needs_web {
        categories.push(ToolCategory::WebFetch);
    }

    // YouTube
    let needs_youtube = combined.contains("youtube")
        || combined.contains("video")
        || combined.contains("transcript")
        || combined.contains("视频")
        || combined.contains("字幕");
    if needs_youtube {
        categories.push(ToolCategory::YouTube);
    }

    // Image generation
    let needs_image_gen = combined.contains("image")
        || combined.contains("picture")
        || combined.contains("图像")
        || combined.contains("图片")
        || combined.contains("generate_image")
        || combined.contains("生成图")
        || combined.contains("画图")
        || combined.contains("绘制")
        || combined.contains("visual")
        || combined.contains("可视化")
        || combined.contains("graph") // knowledge graph often needs images
        || combined.contains("图谱");
    if needs_image_gen {
        categories.push(ToolCategory::ImageGen);
    }

    // Video generation
    let needs_video_gen = combined.contains("generate video")
        || combined.contains("create video")
        || combined.contains("生成视频")
        || combined.contains("视频生成");
    if needs_video_gen {
        categories.push(ToolCategory::VideoGen);
    }

    // Audio generation
    let needs_audio_gen = combined.contains("generate audio")
        || combined.contains("generate music")
        || combined.contains("create music")
        || combined.contains("生成音频")
        || combined.contains("生成音乐");
    if needs_audio_gen {
        categories.push(ToolCategory::AudioGen);
    }

    // Speech/TTS generation
    let needs_speech = combined.contains("speech")
        || combined.contains("tts")
        || combined.contains("text to speech")
        || combined.contains("语音")
        || combined.contains("朗读");
    if needs_speech {
        categories.push(ToolCategory::SpeechGen);
    }

    // Bash/shell execution - detect script execution patterns
    let needs_bash = combined.contains("bash")
        || combined.contains("shell")
        || combined.contains("python3")
        || combined.contains("node ")
        || combined.contains("npm ")
        || combined.contains("scripts/")
        || combined.contains(".py")
        || combined.contains(".sh")
        || combined.contains(".js")
        || combined.contains("$skill_root")
        || combined.contains("command")
        || combined.contains("执行命令")
        || combined.contains("运行脚本")
        || combined.contains("shell命令")
        || combined.contains("格律验证")  // classical-poetry specific
        || combined.contains("格律检查")  // classical-poetry specific
        || combined.contains("poetry_checker")
        || combined.contains("reference_builder");
    if needs_bash {
        categories.push(ToolCategory::Bash);
    }

    // Code execution - detect inline code execution needs
    let needs_code_exec = combined.contains("code_exec")
        || combined.contains("execute code")
        || combined.contains("run code")
        || combined.contains("inline code")
        || combined.contains("代码执行")
        || combined.contains("运行代码")
        || (combined.contains("execute") && (combined.contains("python") || combined.contains("javascript")));
    if needs_code_exec {
        categories.push(ToolCategory::CodeExec);
    }

    // If nothing detected, include essential tools (file_ops is almost always needed)
    if categories.is_empty() {
        categories.push(ToolCategory::FileOps);
    }

    info!(
        inferred_categories = ?categories,
        "Inferred tool categories from skill/request content"
    );

    categories
}

/// Get descriptions for built-in tools
///
/// Returns tool descriptions for the agent prompt so AI knows what tools are available.
/// Includes image generation tool if providers are configured.
pub fn get_builtin_tool_descriptions(
    generation_config: &crate::config::GenerationConfig,
) -> Vec<ToolDescription> {
    use crate::generation::GenerationType;

    let mut tools = vec![
        ToolDescription::with_schema(
            "file_ops",
            "文件系统操作 - 支持 list(列出目录)、read、write、move、copy、delete、mkdir、search、**organize**(一键按类型整理到 Images/Documents/Videos/Audio/Archives/Code/Others)、**batch_move**(批量移动匹配文件)",
            json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["list", "read", "write", "move", "copy", "delete", "mkdir", "search", "batch_move", "organize"],
                        "description": "The file operation to perform"
                    },
                    "path": {
                        "type": "string",
                        "description": "Primary path (source directory for batch_move/organize, target for others)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path (required for move/copy/batch_move operations)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write (required for write operation)"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern for search/batch_move (e.g., '*.pdf', '*.jpg')"
                    }
                },
                "required": ["operation", "path"]
            })
        ),
        ToolDescription::with_schema(
            "search",
            "网络搜索 - 搜索互联网获取最新信息",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    }
                },
                "required": ["query"]
            })
        ),
        ToolDescription::with_schema(
            "web_fetch",
            "获取网页内容 - 读取指定URL的网页内容",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch"
                    }
                },
                "required": ["url"]
            })
        ),
        ToolDescription::with_schema(
            "youtube",
            "YouTube视频信息 - 获取YouTube视频的标题、描述、字幕等信息",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "YouTube video URL"
                    }
                },
                "required": ["url"]
            })
        ),
        ToolDescription::with_schema(
            "bash",
            "Bash命令执行 - 执行bash/shell命令。安全限制：阻止危险命令(sudo, rm -rf /, 等)。默认超时60秒。",
            json!({
                "type": "object",
                "properties": {
                    "cmd": {
                        "type": "string",
                        "description": "Bash command to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory (optional)"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (optional, default 60)"
                    }
                },
                "required": ["cmd"]
            })
        ),
        ToolDescription::with_schema(
            "code_exec",
            "代码执行 - 执行Python/JavaScript/Shell代码。支持语言: python, javascript, shell。安全限制：阻止危险命令。",
            json!({
                "type": "object",
                "properties": {
                    "language": {
                        "type": "string",
                        "enum": ["python", "javascript", "shell"],
                        "description": "Programming language"
                    },
                    "code": {
                        "type": "string",
                        "description": "Code to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory (optional)"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (optional, default 60)"
                    }
                },
                "required": ["language", "code"]
            })
        ),
    ];

    // Add image generation tool if providers are configured
    let all_providers: Vec<_> = generation_config.providers.iter().collect();
    debug!(
        all_providers_count = all_providers.len(),
        "Listing all generation providers for debugging"
    );
    for (name, config) in &all_providers {
        debug!(
            provider = %name,
            enabled = config.enabled,
            capabilities = ?config.capabilities,
            "Generation provider config"
        );
    }

    let image_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Image)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    debug!(
        image_providers_count = image_providers.len(),
        image_providers = ?image_providers,
        "Filtered image providers"
    );

    if !image_providers.is_empty() {
        tools.push(ToolDescription::with_schema(
            "generate_image",
            format!(
                "Image generation - generate images from text descriptions. Available providers: {}. Use the provider parameter to specify which model to use.",
                image_providers.join(", ")
            ),
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Text description of the image to generate"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Image generation provider name"
                    },
                    "model": {
                        "type": "string",
                        "description": "Specific model to use (optional)"
                    }
                },
                "required": ["prompt", "provider"]
            })
        ));
        info!(
            providers = ?image_providers,
            "Added generate_image tool to agent capabilities"
        );
    }

    // Add video generation tool if providers are configured
    let video_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Video)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    if !video_providers.is_empty() {
        tools.push(ToolDescription::with_schema(
            "generate_video",
            format!(
                "Video generation - generate videos from text descriptions. Available providers: {}. Use the provider parameter to specify which model to use.",
                video_providers.join(", ")
            ),
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Text description of the video to generate"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Video generation provider name"
                    },
                    "model": {
                        "type": "string",
                        "description": "Specific model to use (optional)"
                    }
                },
                "required": ["prompt", "provider"]
            })
        ));
        info!(
            providers = ?video_providers,
            "Added generate_video tool to agent capabilities"
        );
    }

    // Add audio generation tool if providers are configured
    let audio_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Audio)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    if !audio_providers.is_empty() {
        tools.push(ToolDescription::with_schema(
            "generate_audio",
            format!(
                "Audio/music generation - generate music or audio from text descriptions. Available providers: {}. Use the provider parameter to specify which model to use.",
                audio_providers.join(", ")
            ),
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Text description of the audio to generate"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Audio generation provider name"
                    },
                    "model": {
                        "type": "string",
                        "description": "Specific model to use (optional)"
                    }
                },
                "required": ["prompt", "provider"]
            })
        ));
        info!(
            providers = ?audio_providers,
            "Added generate_audio tool to agent capabilities"
        );
    }

    // Add speech generation tool if providers are configured
    let speech_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Speech)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    if !speech_providers.is_empty() {
        tools.push(ToolDescription::with_schema(
            "generate_speech",
            format!(
                "Speech/TTS generation - convert text to speech. Available providers: {}. Use the provider parameter to specify which model to use.",
                speech_providers.join(", ")
            ),
            json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text to convert to speech"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Speech generation provider name"
                    },
                    "voice": {
                        "type": "string",
                        "description": "Voice to use (optional)"
                    }
                },
                "required": ["text", "provider"]
            })
        ));
        info!(
            providers = ?speech_providers,
            "Added generate_speech tool to agent capabilities"
        );
    }

    tools
}

/// Filter tool descriptions based on inferred categories
///
/// Only includes tools that match the inferred categories, reducing token usage.
pub fn filter_tools_by_categories(
    all_tools: Vec<ToolDescription>,
    categories: &[ToolCategory],
) -> Vec<ToolDescription> {
    all_tools
        .into_iter()
        .filter(|tool| {
            let name = tool.name.as_str();
            categories.iter().any(|cat| match cat {
                ToolCategory::FileOps => name == "file_ops",
                ToolCategory::Search => name == "search",
                ToolCategory::WebFetch => name == "web_fetch",
                ToolCategory::YouTube => name == "youtube",
                ToolCategory::Bash => name == "bash",
                ToolCategory::CodeExec => name == "code_exec",
                ToolCategory::ImageGen => name == "generate_image",
                ToolCategory::VideoGen => name == "generate_video",
                ToolCategory::AudioGen => name == "generate_audio",
                ToolCategory::SpeechGen => name == "generate_speech",
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_bash_from_script_patterns() {
        // Test classical-poetry skill pattern
        let skill = "python3 $SKILL_ROOT/scripts/poetry_checker.py --mode ci";
        let categories = infer_required_tools(skill, "");
        
        assert!(
            categories.contains(&ToolCategory::Bash),
            "Should detect Bash need from python3 script call"
        );
    }

    #[test]
    fn test_infer_bash_from_poetry_keywords() {
        let skill = "格律验证工具检查平仄";
        let categories = infer_required_tools(skill, "");
        
        assert!(
            categories.contains(&ToolCategory::Bash),
            "Should detect Bash need from poetry validation keywords"
        );
    }

    #[test]
    fn test_infer_bash_from_shell_patterns() {
        let cases = vec![
            "bash scripts/build.sh",
            "执行命令检查格式",
            "运行脚本验证",
            "node scripts/test.js",
        ];

        for case in cases {
            let categories = infer_required_tools(case, "");
            assert!(
                categories.contains(&ToolCategory::Bash),
                "Should detect Bash need from: {}",
                case
            );
        }
    }

    #[test]
    fn test_infer_code_exec() {
        let skill = "execute python code to analyze data";
        let categories = infer_required_tools(skill, "");
        
        assert!(
            categories.contains(&ToolCategory::CodeExec),
            "Should detect CodeExec need from inline execution pattern"
        );
    }

    #[test]
    fn test_filter_tools_includes_bash() {
        use crate::intent::ToolDescription;
        use serde_json::json;

        let tools = vec![
            ToolDescription::with_schema(
                "bash",
                "Execute shell commands",
                json!({"type": "object"})
            ),
            ToolDescription::with_schema(
                "file_ops",
                "File operations",
                json!({"type": "object"})
            ),
        ];

        let categories = vec![ToolCategory::Bash, ToolCategory::FileOps];
        let filtered = filter_tools_by_categories(tools, &categories);

        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().any(|t| t.name == "bash"));
        assert!(filtered.iter().any(|t| t.name == "file_ops"));
    }
}
