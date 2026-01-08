# Design: Skills Capability (Claude Agent Skills Standard)

## Context

Claude Agent Skills 是 Anthropic 发布的开放标准，用于教 Claude 如何完成特定任务。Skills 本质上是**动态 system prompt 注入**，而非可执行代码。

> "Skills are not executable code. They do NOT run Python or JavaScript. Instead of executing discrete actions and returning results, skills inject comprehensive instruction sets that modify how Claude reasons about and approaches the task."

### Stakeholders

- **终端用户**：需要扩展 AI 能力，创建/使用 Skills
- **开发者**：需要与 Claude Code、GitHub Copilot 兼容的格式
- **Aether 架构**：需要与现有 Capability Strategy Pattern 集成

### Constraints

1. **遵循开放标准**：SKILL.md 格式与 Anthropic 规范一致
2. **Strategy Pattern 集成**：作为 `CapabilityStrategy` 实现
3. **低耦合高内聚**：与现有模块（Memory, Search, Video）保持相同设计模式
4. **热加载**：Skills 变更无需重启应用
5. **UI 一致性**：与现有 Settings UI 设计语言保持一致

## Goals / Non-Goals

### Goals

- 实现 Claude Agent Skills 标准（SKILL.md 格式）
- 作为 `CapabilityStrategy` 集成到现有架构
- 支持显式调用（`/skill <name>`）和自动匹配
- 提供内置 Skills（refine-text, translate, summarize）
- **提供 Settings UI 管理 Skills**
- **支持多种安装方式**（官方、URL、ZIP、手动创建）

### Non-Goals

- 工具执行（`allowed-tools` 预留给 MCP 集成）
- Skills 市场/分享平台
- Skills 版本管理
- 多 Skill 组合（MVP 只支持单个 Skill）

---

## Part A: Core Architecture

### System Integration

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              AetherCore                                  │
│                                                                         │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                    CompositeCapabilityExecutor                      │ │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────┐ │ │
│  │  │ Memory   │ │ Search   │ │ MCP      │ │ Video    │ │  Skills   │ │ │
│  │  │ Strategy │ │ Strategy │ │ Strategy │ │ Strategy │ │  Strategy │ │ │
│  │  │ (0)      │ │ (1)      │ │ (2)      │ │ (3)      │ │  (4)      │ │ │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └───────────┘ │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                         │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                       SkillsRegistry                                │ │
│  │  - load_all() / reload()                                           │ │
│  │  - get_skill(id) / list_skills()                                   │ │
│  │  - find_matching(input)                                            │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                         │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                       SkillsInstaller                               │ │
│  │  - install_from_github(url)                                        │ │
│  │  - install_from_zip(path)                                          │ │
│  │  - install_official_skills()                                       │ │
│  │  - create_skill(name, content)                                     │ │
│  │  - delete_skill(id)                                                │ │
│  └────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
```

### Module Structure (Rust)

```
Aether/core/src/
├── skills/                      # Skills module
│   ├── mod.rs                   # Module exports, Skill struct
│   ├── registry.rs              # SkillsRegistry implementation
│   └── installer.rs             # SkillsInstaller implementation (NEW)
│
├── capability/
│   └── strategies/
│       └── skills.rs            # SkillsStrategy
│
├── payload/
│   ├── mod.rs                   # ADD: skill_instructions to AgentContext
│   └── capability.rs            # ADD: Capability::Skills
│
└── config/
    └── mod.rs                   # ADD: SkillsConfig
```

### Data Flow

```
User Input: "/skill refine-text Fix this text"
         │
         ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                              Router                                      │
│  1. Detect /skill command                                               │
│  2. Extract skill_name = "refine-text"                                  │
│  3. Set payload.meta.skill_id = "refine-text"                           │
└──────────────────────────────┬──────────────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                   CompositeCapabilityExecutor                            │
│                                                                         │
│  SkillsStrategy.execute():                                              │
│      1. skill = registry.get_skill(payload.meta.skill_id)               │
│      2. payload.context.skill_instructions = skill.instructions         │
└──────────────────────────────┬──────────────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        PromptAssembler                                   │
│  system_prompt = base + memory + search + video + skill_instructions    │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Part B: Skills Installer (Rust)

### SkillsInstaller Implementation

```rust
use crate::error::{AetherError, Result};
use crate::skills::Skill;
use std::path::PathBuf;
use std::io::{Read, Write};

/// Skills installer for downloading and managing skills
pub struct SkillsInstaller {
    skills_dir: PathBuf,
}

impl SkillsInstaller {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }

    /// Install official skills from anthropics/skills repository
    pub async fn install_official_skills(&self) -> Result<Vec<String>> {
        let url = "https://github.com/anthropics/skills/archive/refs/heads/main.zip";
        self.install_from_github_zip(url, Some("skills/")).await
    }

    /// Install skill from GitHub repository URL
    /// Supports formats:
    /// - https://github.com/user/repo
    /// - github.com/user/repo
    /// - user/repo (assumes GitHub)
    pub async fn install_from_github(&self, url: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_github_url(url)?;
        let zip_url = format!("{}/archive/refs/heads/main.zip", normalized);
        self.install_from_github_zip(&zip_url, None).await
    }

    /// Install from ZIP file (local path)
    pub async fn install_from_zip(&self, zip_path: &PathBuf) -> Result<Vec<String>> {
        let file = std::fs::File::open(zip_path)?;
        let mut archive = zip::ZipArchive::new(file)?;

        let mut installed = Vec::new();

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let name = file.name().to_string();

            // Look for SKILL.md files
            if name.ends_with("SKILL.md") {
                let skill_dir_name = self.extract_skill_dir_name(&name)?;
                let target_dir = self.skills_dir.join(&skill_dir_name);

                // Skip if already exists
                if target_dir.exists() {
                    tracing::info!(skill = %skill_dir_name, "Skill already exists, skipping");
                    continue;
                }

                std::fs::create_dir_all(&target_dir)?;

                let mut content = String::new();
                file.read_to_string(&mut content)?;

                // Validate SKILL.md format
                Skill::parse(&skill_dir_name, &content)?;

                let target_path = target_dir.join("SKILL.md");
                std::fs::write(&target_path, &content)?;

                installed.push(skill_dir_name);
            }
        }

        Ok(installed)
    }

    /// Create a new skill manually
    pub fn create_skill(&self, name: &str, content: &str) -> Result<()> {
        // Validate name (lowercase, hyphens, alphanumeric)
        if !self.is_valid_skill_name(name) {
            return Err(AetherError::invalid_config(
                "Skill name must be lowercase with hyphens only"
            ));
        }

        // Validate content format
        Skill::parse(name, content)?;

        let skill_dir = self.skills_dir.join(name);
        std::fs::create_dir_all(&skill_dir)?;

        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, content)?;

        Ok(())
    }

    /// Update an existing skill
    pub fn update_skill(&self, name: &str, content: &str) -> Result<()> {
        let skill_dir = self.skills_dir.join(name);
        if !skill_dir.exists() {
            return Err(AetherError::not_found(format!("Skill '{}' not found", name)));
        }

        // Validate content format
        Skill::parse(name, content)?;

        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, content)?;

        Ok(())
    }

    /// Delete a skill
    pub fn delete_skill(&self, name: &str) -> Result<()> {
        let skill_dir = self.skills_dir.join(name);
        if !skill_dir.exists() {
            return Err(AetherError::not_found(format!("Skill '{}' not found", name)));
        }

        std::fs::remove_dir_all(&skill_dir)?;
        Ok(())
    }

    // --- Private helpers ---

    fn normalize_github_url(&self, url: &str) -> Result<String> {
        let url = url.trim();

        // Handle short format: user/repo
        if !url.contains("://") && !url.starts_with("github.com") {
            if url.matches('/').count() == 1 {
                return Ok(format!("https://github.com/{}", url));
            }
        }

        // Handle github.com/user/repo
        if url.starts_with("github.com/") {
            return Ok(format!("https://{}", url));
        }

        // Already full URL
        if url.starts_with("https://github.com/") {
            return Ok(url.to_string());
        }

        Err(AetherError::invalid_config("Invalid GitHub URL format"))
    }

    fn extract_skill_dir_name(&self, path: &str) -> Result<String> {
        // Path format: repo-main/skills/skill-name/SKILL.md
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            let parent = parts[parts.len() - 2];
            return Ok(parent.to_string());
        }
        Err(AetherError::invalid_config("Cannot extract skill name from path"))
    }

    fn is_valid_skill_name(&self, name: &str) -> bool {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
            && !name.starts_with('-')
            && !name.ends_with('-')
    }

    async fn install_from_github_zip(
        &self,
        url: &str,
        filter_prefix: Option<&str>
    ) -> Result<Vec<String>> {
        // Download ZIP to temp file
        let response = reqwest::get(url).await?;
        let bytes = response.bytes().await?;

        let temp_dir = std::env::temp_dir();
        let temp_zip = temp_dir.join(format!("aether-skill-{}.zip", uuid::Uuid::new_v4()));
        std::fs::write(&temp_zip, &bytes)?;

        // Install from the downloaded ZIP
        let result = self.install_from_zip(&temp_zip).await;

        // Cleanup
        let _ = std::fs::remove_file(&temp_zip);

        result
    }
}
```

### UniFFI Interface Extension

Add to `aether.udl`:

```idl
// Skill data types
dictionary SkillInfo {
    string id;
    string name;
    string description;
    sequence<string> allowed_tools;
};

// Skills management methods on AetherCore
interface AetherCore {
    // ... existing methods ...

    // Skills Registry
    sequence<SkillInfo> list_skills();
    SkillInfo? get_skill(string id);
    void reload_skills();

    // Skills Installer
    [Async]
    sequence<string> install_official_skills();

    [Async]
    sequence<string> install_skill_from_url(string url);

    [Async]
    sequence<string> install_skill_from_zip(string path);

    [Throws=AetherError]
    void create_skill(string name, string content);

    [Throws=AetherError]
    void update_skill(string name, string content);

    [Throws=AetherError]
    void delete_skill(string id);
};
```

---

## Part C: Skills Settings UI (Swift)

### UI Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                      RootContentView                                     │
│  SettingsTab = .skills                                                  │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐│
│  │                    SkillsSettingsView                                ││
│  │  @StateObject saveBarState (shared)                                 ││
│  │  @State skills: [SkillInfo]                                         ││
│  │  @State searchText: String                                          ││
│  │  @State isLoading: Bool                                             ││
│  │  @State showInstallSheet: Bool                                      ││
│  │  @State showEditorSheet: Bool                                       ││
│  │  @State editingSkill: SkillInfo?                                    ││
│  │                                                                     ││
│  │  ┌─────────────────────────────────────────────────────────────────┐││
│  │  │ Toolbar: [SearchBar] [+ Create] [↓ Install]                     │││
│  │  └─────────────────────────────────────────────────────────────────┘││
│  │                                                                     ││
│  │  ┌─────────────────────────────────────────────────────────────────┐││
│  │  │ ScrollView: Skills List                                         │││
│  │  │  ┌─────────────────────────────────────────────────────────────┐│││
│  │  │  │ SkillCard (refine-text)              [Edit] [Delete]        ││││
│  │  │  └─────────────────────────────────────────────────────────────┘│││
│  │  │  ┌─────────────────────────────────────────────────────────────┐│││
│  │  │  │ SkillCard (translate)                [Edit] [Delete]        ││││
│  │  │  └─────────────────────────────────────────────────────────────┘│││
│  │  │  ...                                                            │││
│  │  └─────────────────────────────────────────────────────────────────┘││
│  │                                                                     ││
│  │  ┌─────────────────────────────────────────────────────────────────┐││
│  │  │ Install Options Card                                            │││
│  │  │  [📦 Official Skills] [🔗 From URL] [📁 Upload ZIP]             │││
│  │  └─────────────────────────────────────────────────────────────────┘││
│  └─────────────────────────────────────────────────────────────────────┘│
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐│
│  │ .sheet: SkillInstallSheet (when showInstallSheet)                  ││
│  │  - URL input field                                                  ││
│  │  - Progress indicator                                               ││
│  │  - Install results                                                  ││
│  └─────────────────────────────────────────────────────────────────────┘│
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐│
│  │ .sheet: SkillEditorPanel (when showEditorSheet)                    ││
│  │  - Name field (readonly if editing)                                 ││
│  │  - Description field                                                ││
│  │  - Allowed tools selector                                           ││
│  │  - Markdown editor (instructions)                                   ││
│  │  - Preview toggle                                                   ││
│  │  - [Cancel] [Save]                                                  ││
│  └─────────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────────┘
```

### Component Structure (Swift)

```
Aether/Sources/
├── SettingsView.swift              # ADD: SettingsTab.skills case
├── SkillsSettingsView.swift        # NEW: Main skills settings view
│
├── Components/
│   ├── Molecules/
│   │   └── SkillCard.swift         # NEW: Skill list item card
│   │
│   └── Organisms/
│       ├── SkillEditorPanel.swift  # NEW: Create/Edit skill sheet
│       └── SkillInstallSheet.swift # NEW: Install skill sheet
```

### SkillsSettingsView Design

```swift
import SwiftUI

struct SkillsSettingsView: View {
    @ObservedObject var saveBarState: SettingsSaveBarState

    // State
    @State private var skills: [SkillInfo] = []
    @State private var searchText: String = ""
    @State private var isLoading: Bool = false
    @State private var errorMessage: String?

    // Sheets
    @State private var showInstallSheet: Bool = false
    @State private var showEditorSheet: Bool = false
    @State private var editingSkill: SkillInfo? = nil

    var filteredSkills: [SkillInfo] {
        if searchText.isEmpty {
            return skills
        }
        return skills.filter {
            $0.name.localizedCaseInsensitiveContains(searchText) ||
            $0.description.localizedCaseInsensitiveContains(searchText)
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Toolbar
                toolbarSection

                // Skills List
                skillsListSection

                // Install Options
                installOptionsSection
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .onAppear {
            loadSkills()
            saveBarState.reset()
        }
        .sheet(isPresented: $showInstallSheet) {
            SkillInstallSheet(onInstalled: loadSkills)
        }
        .sheet(isPresented: $showEditorSheet) {
            SkillEditorPanel(
                skill: editingSkill,
                onSave: { name, content in
                    saveSkill(name: name, content: content)
                },
                onCancel: {
                    showEditorSheet = false
                    editingSkill = nil
                }
            )
        }
    }

    // MARK: - Sections

    private var toolbarSection: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            SearchBar(text: $searchText, placeholder: L("settings.skills.search"))

            Spacer()

            Button(action: {
                editingSkill = nil
                showEditorSheet = true
            }) {
                Label(L("settings.skills.create"), systemImage: "plus")
            }
            .buttonStyle(.borderedProminent)

            Button(action: { showInstallSheet = true }) {
                Label(L("settings.skills.install"), systemImage: "arrow.down.circle")
            }
            .buttonStyle(.bordered)
        }
    }

    private var skillsListSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Label(L("settings.skills.installed"), systemImage: "book.closed")
                .font(DesignTokens.Typography.heading)

            if isLoading {
                ProgressView()
                    .frame(maxWidth: .infinity)
                    .padding()
            } else if filteredSkills.isEmpty {
                emptyStateView
            } else {
                ForEach(filteredSkills, id: \.id) { skill in
                    SkillCard(
                        skill: skill,
                        onEdit: {
                            editingSkill = skill
                            showEditorSheet = true
                        },
                        onDelete: {
                            deleteSkill(skill)
                        }
                    )
                }
            }
        }
    }

    private var installOptionsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.skills.install_options"), systemImage: "square.and.arrow.down")
                .font(DesignTokens.Typography.heading)

            HStack(spacing: DesignTokens.Spacing.md) {
                InstallOptionButton(
                    title: L("settings.skills.official"),
                    icon: "building.columns",
                    description: L("settings.skills.official_desc"),
                    action: installOfficialSkills
                )

                InstallOptionButton(
                    title: L("settings.skills.from_url"),
                    icon: "link",
                    description: L("settings.skills.from_url_desc"),
                    action: { showInstallSheet = true }
                )

                InstallOptionButton(
                    title: L("settings.skills.upload_zip"),
                    icon: "doc.zipper",
                    description: L("settings.skills.upload_zip_desc"),
                    action: uploadZipFile
                )
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium))
    }

    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "book.closed")
                .font(.system(size: 48))
                .foregroundColor(.secondary)
            Text(L("settings.skills.empty"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(.secondary)
            Button(L("settings.skills.install_first")) {
                showInstallSheet = true
            }
            .buttonStyle(.borderedProminent)
        }
        .frame(maxWidth: .infinity)
        .padding(DesignTokens.Spacing.xl)
    }

    // MARK: - Actions

    private func loadSkills() {
        isLoading = true
        Task {
            do {
                let core = AetherCore.shared
                skills = try await core.listSkills()
            } catch {
                errorMessage = error.localizedDescription
            }
            isLoading = false
        }
    }

    private func installOfficialSkills() {
        isLoading = true
        Task {
            do {
                let core = AetherCore.shared
                let installed = try await core.installOfficialSkills()
                await MainActor.run {
                    loadSkills()
                    ToastManager.shared.show(
                        message: L("settings.skills.installed_count", installed.count)
                    )
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                }
            }
            isLoading = false
        }
    }

    private func uploadZipFile() {
        let panel = NSOpenPanel()
        panel.title = L("settings.skills.select_zip")
        panel.allowedContentTypes = [.zip]
        panel.allowsMultipleSelection = false

        guard panel.runModal() == .OK, let url = panel.url else { return }

        isLoading = true
        Task {
            do {
                let core = AetherCore.shared
                let installed = try await core.installSkillFromZip(path: url.path)
                await MainActor.run {
                    loadSkills()
                    ToastManager.shared.show(
                        message: L("settings.skills.installed_count", installed.count)
                    )
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                }
            }
            isLoading = false
        }
    }

    private func saveSkill(name: String, content: String) {
        Task {
            do {
                let core = AetherCore.shared
                if editingSkill != nil {
                    try await core.updateSkill(name: name, content: content)
                } else {
                    try await core.createSkill(name: name, content: content)
                }
                await MainActor.run {
                    loadSkills()
                    showEditorSheet = false
                    editingSkill = nil
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                }
            }
        }
    }

    private func deleteSkill(_ skill: SkillInfo) {
        Task {
            do {
                let core = AetherCore.shared
                try await core.deleteSkill(id: skill.id)
                await MainActor.run {
                    loadSkills()
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                }
            }
        }
    }
}
```

### SkillCard Component

```swift
struct SkillCard: View {
    let skill: SkillInfo
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var isHovered: Bool = false
    @State private var showDeleteConfirm: Bool = false

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Icon
            Image(systemName: skillIcon)
                .font(.system(size: 24))
                .foregroundColor(DesignTokens.Colors.accentBlue)
                .frame(width: 40, height: 40)
                .background(DesignTokens.Colors.accentBlue.opacity(0.1))
                .clipShape(RoundedRectangle(cornerRadius: 8))

            // Info
            VStack(alignment: .leading, spacing: 4) {
                Text(skill.name)
                    .font(DesignTokens.Typography.heading)
                Text(skill.description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.secondary)
                    .lineLimit(2)
            }

            Spacer()

            // Actions (visible on hover)
            if isHovered {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Button(action: onEdit) {
                        Image(systemName: "pencil")
                    }
                    .buttonStyle(.borderless)

                    Button(action: { showDeleteConfirm = true }) {
                        Image(systemName: "trash")
                            .foregroundColor(DesignTokens.Colors.error)
                    }
                    .buttonStyle(.borderless)
                }
                .transition(.opacity)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(isHovered ? DesignTokens.Colors.cardBackground : Color.clear)
        )
        .onHover { isHovered = $0 }
        .confirmationDialog(
            L("settings.skills.delete_confirm"),
            isPresented: $showDeleteConfirm,
            titleVisibility: .visible
        ) {
            Button(L("common.delete"), role: .destructive, action: onDelete)
            Button(L("common.cancel"), role: .cancel) {}
        }
    }

    private var skillIcon: String {
        switch skill.id {
        case "refine-text": return "text.quote"
        case "translate": return "globe"
        case "summarize": return "doc.plaintext"
        default: return "book.closed"
        }
    }
}
```

### SkillEditorPanel Component

```swift
struct SkillEditorPanel: View {
    let skill: SkillInfo?
    let onSave: (String, String) -> Void
    let onCancel: () -> Void

    @State private var name: String = ""
    @State private var description: String = ""
    @State private var allowedTools: [String] = []
    @State private var instructions: String = ""
    @State private var showPreview: Bool = false

    private var isEditing: Bool { skill != nil }

    private var generatedContent: String {
        """
        ---
        name: \(name)
        description: \(description)
        allowed-tools: [\(allowedTools.joined(separator: ", "))]
        ---

        \(instructions)
        """
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text(isEditing ? L("settings.skills.edit") : L("settings.skills.create"))
                    .font(DesignTokens.Typography.title)
                Spacer()
                Button(action: onCancel) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding()

            Divider()

            // Form
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    // Name field
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        Text(L("settings.skills.name"))
                            .font(DesignTokens.Typography.caption)
                        TextField(L("settings.skills.name_placeholder"), text: $name)
                            .textFieldStyle(.roundedBorder)
                            .disabled(isEditing)
                    }

                    // Description field
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        Text(L("settings.skills.description"))
                            .font(DesignTokens.Typography.caption)
                        TextField(L("settings.skills.description_placeholder"), text: $description)
                            .textFieldStyle(.roundedBorder)
                    }

                    // Instructions (Markdown editor)
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        HStack {
                            Text(L("settings.skills.instructions"))
                                .font(DesignTokens.Typography.caption)
                            Spacer()
                            Toggle(L("settings.skills.preview"), isOn: $showPreview)
                                .toggleStyle(.switch)
                                .labelsHidden()
                            Text(L("settings.skills.preview"))
                                .font(DesignTokens.Typography.caption)
                        }

                        if showPreview {
                            // Markdown preview
                            ScrollView {
                                Text(try! AttributedString(markdown: instructions))
                                    .textSelection(.enabled)
                            }
                            .frame(minHeight: 200)
                            .padding(DesignTokens.Spacing.md)
                            .background(DesignTokens.Colors.cardBackground)
                            .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small))
                        } else {
                            // Text editor
                            TextEditor(text: $instructions)
                                .font(DesignTokens.Typography.code)
                                .frame(minHeight: 200)
                                .padding(DesignTokens.Spacing.sm)
                                .background(DesignTokens.Colors.cardBackground)
                                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small))
                        }
                    }
                }
                .padding()
            }

            Divider()

            // Footer
            HStack {
                Spacer()
                Button(L("common.cancel"), action: onCancel)
                    .buttonStyle(.bordered)
                Button(L("common.save")) {
                    onSave(name, generatedContent)
                }
                .buttonStyle(.borderedProminent)
                .disabled(name.isEmpty || description.isEmpty)
            }
            .padding()
        }
        .frame(width: 600, height: 500)
        .onAppear {
            if let skill = skill {
                name = skill.name
                description = skill.description
                // Parse instructions from existing skill content
                // (would need to load full content from core)
            }
        }
    }
}
```

---

## Decisions

### Decision 1: Capability 优先级

**选择**：`Skills = 4`（在 Video 之后）

**权衡**：
- 优点：Skills 可以引用其他 Capability 的上下文
- 缺点：Skills 执行时间稍晚

**理由**：Skills 是指令增强，应在所有上下文收集完成后执行

### Decision 2: 安装方式实现

**选择**：HTTP 下载 ZIP + 本地解压

**权衡**：
- 方案 A：使用 git clone（需要 git 依赖）
- 方案 B：下载 ZIP（无外部依赖）

**理由**：方案 B 更轻量，不需要用户安装 git

### Decision 3: UI 放置位置

**选择**：独立的 Settings Tab（.skills）

**权衡**：
- 方案 A：嵌入 General Settings
- 方案 B：独立 Tab

**理由**：Skills 功能足够重要，值得独立 Tab；与 Providers、Search 等平级

### Decision 4: 编辑器设计

**选择**：模态 Sheet

**权衡**：
- 方案 A：侧边面板（如 ProvidersView）
- 方案 B：模态 Sheet

**理由**：Skill 编辑需要较大空间（Markdown 编辑器），Sheet 提供更好的焦点

### Decision 5: 删除确认

**选择**：使用 confirmationDialog

**权衡**：
- 方案 A：直接删除
- 方案 B：确认对话框

**理由**：防止误删，与 macOS 设计规范一致

---

## Risks / Trade-offs

### Risk 1: GitHub API 限制

**缓解**：
- 使用 ZIP 下载而非 API
- 缓存下载结果
- 显示清晰的错误信息

### Risk 2: ZIP 格式不标准

**缓解**：
- 验证 SKILL.md 存在
- 跳过无效的 skill 目录
- 日志记录跳过原因

### Risk 3: 名称冲突

**缓解**：
- 跳过已存在的 skill（不覆盖）
- 显示哪些被跳过
- 允许用户手动删除后重新安装

### Risk 4: Markdown 注入

**缓解**：
- Skill 指令只注入到 system prompt
- 不执行任何代码
- 用户可以查看完整 SKILL.md

---

## Open Questions

1. **Q: 是否支持 Skill 版本管理？**
   - A: MVP 不支持。后续可通过 manifest.json 添加版本信息。

2. **Q: 是否显示 Skill 来源？**
   - A: MVP 不追踪来源。后续可添加 metadata 字段记录来源。

3. **Q: 是否支持批量删除？**
   - A: MVP 只支持单个删除。后续可添加多选功能。

4. **Q: 如何处理大型 ZIP 文件？**
   - A: 添加进度指示器；设置合理的超时。
