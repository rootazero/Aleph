# Form Components Usage Examples

本文档展示如何使用可复用的表单组件来构建设置页面。

## 导入组件

```rust
use crate::components::{
    SettingsSection, FormField, TextInput, SelectInput,
    NumberInput, SwitchInput, SaveButton, ErrorMessageDynamic
};
```

## 基本示例

### 1. 设置区块 (SettingsSection)

```rust
view! {
    <SettingsSection
        title="Language"
        description="Configure interface language"
    >
        // 表单字段放在这里
    </SettingsSection>
}
```

### 2. 文本输入 (TextInput)

```rust
let (value, set_value) = signal(String::from(""));

view! {
    <FormField label="Email" help_text="Your email address">
        <TextInput
            value=value.into()
            on_change=move |v| set_value.set(v)
            placeholder="user@example.com"
        />
    </FormField>
}
```

### 3. 下拉选择 (SelectInput)

```rust
let (language, set_language) = signal(String::from("en"));

view! {
    <FormField label="Language">
        <SelectInput
            value=language.into()
            on_change=move |v| set_language.set(v)
            options=vec![
                ("en", "English"),
                ("zh-Hans", "简体中文"),
                ("ja", "日本語"),
            ]
        />
    </FormField>
}
```

### 4. 数字输入 + 滑块 (NumberInput)

```rust
let (speed, set_speed) = signal(100);

view! {
    <FormField label="Typing Speed" help_text="Characters per second">
        <NumberInput
            value=speed.into()
            on_change=move |v| set_speed.set(v)
            min=50
            max=400
            step=Some(10)
            show_slider=true
            suffix=Some("cps")
        />
    </FormField>
}
```

### 5. 开关 (SwitchInput)

```rust
let (enabled, set_enabled) = signal(false);

view! {
    <FormField label="Auto Start">
        <SwitchInput
            checked=enabled.into()
            on_change=move |v| set_enabled.set(v)
            label=Some("Launch on system startup")
        />
    </FormField>
}
```

### 6. 保存按钮 (SaveButton)

```rust
let (saving, set_saving) = signal(false);

view! {
    <SaveButton
        on_click=move || {
            set_saving.set(true);
            // 执行保存逻辑
        }
        loading=saving.into()
        text=Some("Save Changes")
    />
}
```

### 7. 错误提示 (ErrorMessageDynamic)

```rust
let (error, set_error) = signal(Option::<String>::None);

view! {
    <ErrorMessageDynamic error=error.into() />
}
```

## 完整示例：语言设置页面

```rust
use leptos::prelude::*;
use crate::components::*;
use crate::context::DashboardState;
use crate::api::ConfigApi;

#[component]
pub fn LanguageSettingsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // 状态管理
    let (language, set_language) = signal(String::from("system"));
    let (saving, set_saving) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // 保存逻辑
    let save_config = move || {
        set_saving.set(true);
        set_error.set(None);

        spawn_local(async move {
            match ConfigApi::update_language(&state, language.get()).await {
                Ok(_) => {
                    set_saving.set(false);
                }
                Err(e) => {
                    set_saving.set(false);
                    set_error.set(Some(e));
                }
            }
        });
    };

    view! {
        <div class="p-8 max-w-4xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2">
                    "Language Settings"
                </h1>
                <p class="text-slate-400">
                    "Configure interface language preferences"
                </p>
            </div>

            <ErrorMessageDynamic error=error.into() />

            <SettingsSection
                title="Interface Language"
                description="Choose your preferred language"
            >
                <FormField label="Language">
                    <SelectInput
                        value=language.into()
                        on_change=move |v| {
                            set_language.set(v);
                            save_config();
                        }
                        options=vec![
                            ("system", "System Default"),
                            ("en", "English"),
                            ("zh-Hans", "简体中文"),
                            ("zh-Hant", "繁體中文"),
                            ("ja", "日本語"),
                            ("ko", "한국어"),
                        ]
                    />
                </FormField>
            </SettingsSection>

            <div class="mt-6">
                <SaveButton
                    on_click=save_config
                    loading=saving.into()
                />
            </div>
        </div>
    }
}
```

## 设计原则

### 1. 一致的视觉风格
所有组件使用统一的颜色方案和间距：
- 背景：`bg-slate-900/50` 或 `bg-slate-800`
- 边框：`border-slate-800` 或 `border-slate-700`
- 文字：`text-slate-200` (主要) / `text-slate-400` (次要)
- 焦点：`focus:ring-indigo-500`

### 2. 响应式设计
- 使用 `w-full` 确保表单字段占满容器宽度
- 使用 `max-w-4xl` 限制内容最大宽度
- 使用 `space-y-*` 提供一致的垂直间距

### 3. 可访问性
- 所有输入字段都有对应的 `<label>`
- 使用语义化的 HTML 元素
- 提供清晰的帮助文本和错误提示

### 4. 状态管理
- 使用 Leptos 的 `signal()` 管理本地状态
- 使用 `spawn_local` 处理异步操作
- 提供 loading 和 error 状态反馈

## 组件 API 参考

### SettingsSection
- `title: &'static str` - 区块标题
- `description: Option<&'static str>` - 可选描述
- `children: Children` - 子内容

### FormField
- `label: &'static str` - 字段标签
- `help_text: Option<&'static str>` - 可选帮助文本
- `children: Children` - 表单控件

### TextInput
- `value: Signal<String>` - 当前值
- `on_change: impl Fn(String)` - 变更回调
- `placeholder: Option<&'static str>` - 占位符
- `input_type: Option<&'static str>` - 输入类型 (默认: "text")
- `monospace: bool` - 是否使用等宽字体

### SelectInput
- `value: Signal<String>` - 当前值
- `on_change: impl Fn(String)` - 变更回调
- `options: Vec<(&'static str, &'static str)>` - 选项 (值, 标签)

### NumberInput
- `value: Signal<i32>` - 当前值
- `on_change: impl Fn(i32)` - 变更回调
- `min: i32` - 最小值
- `max: i32` - 最大值
- `step: Option<i32>` - 步长
- `show_slider: bool` - 是否显示滑块
- `suffix: Option<&'static str>` - 值后缀

### SwitchInput
- `checked: Signal<bool>` - 当前状态
- `on_change: impl Fn(bool)` - 变更回调
- `label: Option<&'static str>` - 可选标签

### SaveButton
- `on_click: impl Fn()` - 点击回调
- `loading: Signal<bool>` - 加载状态
- `text: Option<&'static str>` - 按钮文本 (默认: "Save")

### ErrorMessageDynamic
- `error: Signal<Option<String>>` - 错误消息

## 最佳实践

1. **使用 FormField 包装所有输入**
   ```rust
   <FormField label="Name" help_text="Your full name">
       <TextInput ... />
   </FormField>
   ```

2. **提供即时反馈**
   ```rust
   on_change=move |v| {
       set_value.set(v);
       save_config(); // 自动保存
   }
   ```

3. **处理错误状态**
   ```rust
   match result {
       Ok(_) => { /* 成功 */ }
       Err(e) => set_error.set(Some(e)),
   }
   ```

4. **使用语义化的标签**
   - 使用清晰、描述性的标签文本
   - 提供有用的帮助文本
   - 使用合适的输入类型 (email, number, etc.)
