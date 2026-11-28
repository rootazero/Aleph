# Severed-wire audit — slice `s2-gen-providers`

Repo: `/home/zou/data/workspace/Aleph` (branch `main`). Audit date: 2026-11-28.
Slice key: `s2-gen-providers`. Brief: `.superpowers/sdd/2026-11-28-severed-wire-audit/AUDITOR_BRIEF.md`.

Slice files (all read in full before any judgement was formed):

- `src/generation/providers/mod.rs` (81 lines)
- `src/generation/providers/factory.rs` (533 lines)
- `src/generation/providers/http.rs` (187 lines)
- `src/generation/providers/url_normalize.rs` (202 lines)
- `src/generation/providers/google_imagen.rs` (975 lines)
- `src/generation/providers/openai_image.rs` (959 lines)
- `src/generation/providers/stability.rs` (1012 lines)
- `src/generation/providers/tests.rs` (725 lines, test-only)

Cross-slice files read to establish the far end of a wire (not slice files, cited as the
consumer side): `src/generation/mod.rs`, `src/generation/types/params.rs`,
`src/generation/registry.rs`, `src/generation/providers/openai_compat/{builder,generate,helpers}.rs`,
`src/config/types/generation/{mod,provider,defaults,config}.rs`,
`src/config/types/generation/presets/registry.rs`, `src/config/types/voice_local.rs`,
`src/gateway/handlers/generation_providers/*`, `src/gateway/voice/local_provider.rs`,
`src/builtin_tools/generation/image_generate.rs`,
`src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs`.

Production/test boundary used throughout: a reader inside `#[cfg(test)] mod tests`
(or a `tests.rs` file, or `tests/` integration dir) is **not** a production consumer.

---

## Section A — Findings

| id | form | severity | producer (file:line) | consumer (file:line) | evidence | required outcome |
|----|------|----------|----------------------|----------------------|----------|------------------|
| SW-S2-GEN-PROVIDERS-1 | 3 | high | `src/config/types/generation/defaults.rs:172` — `GenerationDefaults::to_params()` | **no consumer found** (only `src/config/types/generation/mod.rs:259`, inside `#[cfg(test)] mod tests` at `:27`) | `rg -n --glob '*.rs' '\bto_params\b' src/config/` → 2 hits (def + one test call). `rg -n --glob '*.rs' 'defaults\.(width\|height\|aspect_ratio\|quality\|style\|n\|duration_seconds\|fps\|speed\|language\|guidance_scale\|steps)\b' .` → only config-struct/test hits + the Panel form write. `src/builtin_tools/generation/image_generate.rs:138-146` builds `GenerationParams` from tool args only. | CONNECT |
| SW-S2-GEN-PROVIDERS-2 | 5 | medium | `src/generation/providers/openai_compat/builder.rs:184-186` — endpoint = `resolved.primary_endpoint(self.supported_types[0])` | `src/generation/providers/openai_compat/generate.rs:72` — `let url = self.generations_url();` (one endpoint for image **and** video) | read of `builder.rs:180-186` + `generate.rs:55-72`; `rg -n 'generations_url' src/generation/providers/openai_compat/` → single endpoint helper for all modalities | CONNECT |
| SW-S2-GEN-PROVIDERS-3 | 1 | medium | `src/generation/providers/openai_image.rs:169` — `pub fn edits_url(&self) -> Option<String>` | **no consumer found** | `rg -n --glob '*.rs' '\bedits_url\b' .` → 6 hits: the definition + 5 that belong to a **different** type (`pub(crate) fn edits_url` at `src/generation/providers/openai_compat/helpers.rs:44`, used at `openai_compat/edit.rs:198`, `openai_compat/mod.rs:717,733,744`) | DECIDE |
| SW-S2-GEN-PROVIDERS-4 | 1 | low | `src/generation/providers/google_imagen.rs:602` — `pub fn is_valid_aspect_ratio`, `:608` — `pub fn is_valid_image_size` | **no consumer found** | `rg -n --glob '*.rs' '\b(is_valid_aspect_ratio\|is_valid_image_size)\b' .` → 19 hits: 11 in `google_imagen.rs`'s own `#[cfg(test)] mod tests` (`:612-975`), 8 in `google_veo`'s **separate** copies (`google_veo/helpers.rs:11`, `google_veo/provider.rs:242,249`, `google_veo/mod.rs:49,268-272`) | CUT |
| SW-S2-GEN-PROVIDERS-5 | 1 | low | `src/generation/providers/google_imagen.rs:68` — `pub const IMAGE_SIZES: &[&str] = &["1K","2K"]` | **no consumer found** | `rg -n --glob '*.rs' '\bIMAGE_SIZES\b' .` → 2 hits: the definition (`:68`) and `google_imagen.rs:609` inside the dead `is_valid_image_size` (see SW-S2-GEN-PROVIDERS-4) | CUT |
| SW-S2-GEN-PROVIDERS-6 | 3 | medium | `src/config/types/generation/provider.rs:88` — `model_aliases`; `:177` — `pub fn resolve_model` | `model_aliases`: `src/generation/providers/factory.rs:304` (replicate arm only). `resolve_model`: **no consumer found** | `rg -n --glob '*.rs' '\bresolve_model\b' .` → the config accessor is called only at `src/config/types/generation/mod.rs:196,200,205` (test) and `tests/provider_config_probe/serde_tests.rs:328-330` (integration test); the other 6 hits are `ReplicateProvider::resolve_model` (`openai_compat`-unrelated, `replicate/provider.rs:99`), a different type | DECIDE |
| SW-S2-GEN-PROVIDERS-7 | 5 | low | `src/generation/providers/factory.rs:30-40` — “Supported Provider Types” doc; `src/generation/providers/mod.rs:7-16` — “Available Providers” doc | n/a (the contradicting source of truth is `factory.rs:126-411` arms + `factory.rs:404-408` catch-all) | the doc lists 9 provider groups / 18 aliases; the match has 20 arms / 34 aliases (compare `factory.rs:404-408`, which enumerates all 34 correctly) | CUT |
| SW-S2-GEN-PROVIDERS-8 | 3 | medium | `src/config/types/generation/provider.rs` `color` field → applied at `src/generation/providers/factory.rs:248` (openai_compat), `:348` (fal), `:385` (midjourney) only | 16 of 20 arms ignore it; hardcoded colours at `google_imagen.rs:589`, `openai_image.rs:530`, `stability.rs:542`; and `GenerationProvider::color()` itself has **no production reader** | `rg -n --glob '*.rs' '\.color\(\)' .` → 6 hits, all `AiProvider::color` (`src/providers/metering.rs:423`, `model_override_provider.rs:78`, `think_level_provider.rs:83`, `interfaces/tui/.../markdown.rs:471`) — a different trait. UI colour comes from the config wire (`src/gateway/handlers/generation_providers/wire.rs:96,123`), never from the provider object. | DECIDE |
| SW-S2-GEN-PROVIDERS-9 | 3 | medium | `src/config/types/generation/provider.rs:96` — `edit_url` | `src/generation/providers/factory.rs:253-255` — the **`openai_compat` arm only** | `rg -n --glob '*.rs' 'edit_url' src/` → `factory.rs:253,254` + `openai_compat/helpers.rs:45,46` + config struct + gateway wire (`generation_providers/mod.rs:98,153`, `wire.rs:105,133`); no other factory arm reads it. Panel exposes the field for **every** custom provider (`interfaces/webchat/src/platform/wide/views/settings/generation_providers/add_custom.rs:250-251`, `detail_view.rs:381-382`). | DECIDE |

### SW-S2-GEN-PROVIDERS-1 — `[generation.providers.X.defaults]` image/video/audio knobs never reach a request

**Producer.** `GenerationDefaults` (`src/config/types/generation/defaults.rs:27-93`) is parsed from
`[generation.providers.<name>.defaults]` and is documented with a TOML example at
`defaults.rs:14-26` (`width = 1024`, `height = 1024`, `quality = "hd"`, `style = "vivid"`, `n = 1`).
It is validated (`defaults.rs:96-168`) and the Panel writes it back
(`interfaces/webchat/src/platform/wide/views/settings/generation_providers/detail_view.rs:106-108`).

**Consumer.** The single function that could carry the defaults into a request is
`GenerationDefaults::to_params()` (`src/config/types/generation/defaults.rs:172-217`), which maps
all fourteen fields onto `GenerationParams`. Its only caller in the whole repository is
`src/config/types/generation/mod.rs:259`, which lives inside `#[cfg(test)] mod tests`
(`mod tests` starts at `src/config/types/generation/mod.rs:27`).

**What is missing.** The production path never calls `to_params()`. The image tool builds the
request params straight from its own tool arguments:
`src/builtin_tools/generation/image_generate.rs:138-146`
(`GenerationParams { width: args.width, height: args.height, quality: args.quality, style: args.style, ..Default::default() }`).
The factory reads exactly one field of the whole struct — `config.defaults.voice`
(`factory.rs:139, 209, 219, 229, 331`) — plus `config.defaults.format`, read once outside the
provider layer at `src/gateway/voice/local_provider.rs:156`.

**Why it matters.** A parsed, validated, documented, UI-editable knob that is never read by any
production code path: `[generation.providers.dalle.defaults] width = 1024` (or `quality`, `style`,
`n`, `aspect_ratio`, `duration_seconds`, `fps`, `speed`, `language`, `guidance_scale`, `steps`)
is silently a no-op. The request goes out with whatever the model put in the tool call. This is
the shape the brief calls “looks present but is dark”: the config schema, the validator and the
Panel form all advertise a default that no request ever carries.

**Minimal fix (CONNECT).** In the generation tool layer, merge the selected provider’s
`config.defaults.to_params()` **under** the tool-supplied params (the struct already has a
merge helper — `src/generation/types/params.rs:112-117` / `src/generation/types/mod.rs:99-105`).
Alternatively, if per-provider defaults are not a product requirement, CUT the twelve unused
fields plus the `defaults.rs:14-26` TOML example so the schema stops lying.

### SW-S2-GEN-PROVIDERS-2 — `openai_compat` derives one endpoint per provider but sends every modality to it

**Producer.** `ResolvedUrl::primary_endpoint` (`src/generation/providers/url_normalize.rs:22-35`)
returns exactly one endpoint for a given generation type. The openai_compat builder calls it once,
with the **first** supported type:
`src/generation/providers/openai_compat/builder.rs:183-186`
(`let gen_type = self.supported_types[0]; … let endpoint = resolved.primary_endpoint(gen_type);`).
The builder’s `supported_types` is the config’s `capabilities` verbatim
(`src/generation/providers/factory.rs:256-259`).

**Consumer.** `src/generation/providers/openai_compat/generate.rs:55-72` builds an image body or a
video body and then, for **both**, takes `let url = self.generations_url();` — a helper that
returns the single stored `self.endpoint`
(`src/generation/providers/openai_compat/helpers.rs:15-17`).

**What is missing.** A per-modality endpoint. There is no `primary_endpoint(request.generation_type)`
at request time, and no second endpoint field on the provider.

**Why it matters.** A provider configured as
`provider_type = "openai_compat"`, `capabilities = ["image","video"]`,
`base_url = "https://api.example.com"` resolves the base to `ResolvedUrl::Standard`
(`url_normalize.rs:57-70`), so the endpoint becomes
`https://api.example.com/v1/images/generations` — and the **video** request is POSTed there too.
With `capabilities = ["video","image"]` the inversion happens: the image request goes to
`/v1/videos/generations`. The built-in presets escape this only by luck: every openai_compat
preset carries a *full* custom URL (`src/config/types/generation/presets/registry.rs:59-61, 72-74,
85-87, 98-100, 111-113, 133-135`), which `resolve_base_url` classifies as `ResolvedUrl::Custom`
and returns verbatim (`url_normalize.rs:22-24`). A hand-written provider in `config.toml` does not
get that escape hatch.

Note the video suffix itself (`/v1/videos/generations`, `url_normalize.rs:28`) matches no
protocol this repository implements: openai_compat’s video body is the **Ark task** shape
(`generate.rs:55-62` → `build_video_task_body`), whose real endpoint is the preset’s
`/api/v3/contents/generations/tasks`.

**Minimal fix (CONNECT).** Resolve the endpoint per request instead of per provider: store the
`ResolvedUrl` on the provider and call `primary_endpoint(request.generation_type)` inside
`generate()`; or keep a per-type endpoint map on the provider for the types it declares.

### SW-S2-GEN-PROVIDERS-3 — `OpenAiImageProvider::edits_url()` has no consumer; the DALL·E edits chain is dark end to end

**Producer.** `src/generation/providers/openai_image.rs:168-171`

```rust
/// Get the image edits endpoint URL.
/// Returns None for custom URLs or when secondary endpoint is unavailable.
#[must_use]
pub fn edits_url(&self) -> Option<String> {
    self.resolved.secondary_endpoint(GenerationType::Image)
}
```

**Consumer.** None. `rg -n --glob '*.rs' '\bedits_url\b' .` returns six hits: the definition at
`openai_image.rs:169`, and five that belong to a **different** type — `pub(crate) fn edits_url` on
`OpenAiCompatProvider` at `src/generation/providers/openai_compat/helpers.rs:44`, consumed at
`openai_compat/edit.rs:198` and `openai_compat/mod.rs:717,733,744`. The two ends never meet.

**What is missing.** The consumer of the *only* remaining reader of the secondary-endpoint
machinery. `ResolvedUrl::secondary_endpoint` (`src/generation/providers/url_normalize.rs:41-51`)
has exactly one reader in the whole repository — the dead method above. So the entire
“secondary endpoint” concept is dark.

**Why it matters.** `OpenAiImageProvider` does not override `supports_image_editing`
(`src/generation/mod.rs:342` default `false`) nor `edit_image` (`src/generation/mod.rs:320-332`
default = error). The `openai` / `openai_image` / `dalle` provider types therefore cannot edit
images at all, and the public `edits_url()` accessor on the provider advertises a capability the
provider does not have. A caller that trusted the accessor and tried to use the URL would also
hit a provider that refuses the request.

**Minimal fix (DECIDE).** Either CUT — delete `edits_url()` and, if nothing else needs it,
`ResolvedUrl::secondary_endpoint` — or CONNECT: implement `supports_image_editing()`/`edit_image()`
on `OpenAiImageProvider` and have the factory feed it `config.edit_url` (which today only the
`openai_compat` arm consumes — see SW-S2-GEN-PROVIDERS-9).

### SW-S2-GEN-PROVIDERS-4 — `google_imagen::is_valid_aspect_ratio` / `is_valid_image_size` are exported but never used in production

**Producer.** `src/generation/providers/google_imagen.rs:600-610` — two `pub fn` validators over
the module’s allow-lists.

**Consumer.** None outside tests. `rg -n --glob '*.rs' '\b(is_valid_aspect_ratio|is_valid_image_size)\b' .`
returns 19 hits, all of which are either (a) the eleven assertions inside
`google_imagen.rs`’s own `#[cfg(test)] mod tests` (`:612-975`, assertions at `:792-800` and
`:805-809`), or (b) `google_veo`’s **own, separate** `is_valid_aspect_ratio`
(`src/generation/providers/google_veo/helpers.rs:11`) — a different function over a different
value set (`google_veo/constants.rs:28` = `["16:9","9:16"]` vs
`google_imagen.rs:65` = `["1:1","3:4","4:3","9:16","16:9"]`). The two names agree; the two
functions are unrelated.

**What is missing.** The production call site. `determine_aspect_ratio`
(`google_imagen.rs:228-250`) re-implements the check inline instead of calling the exported
validator: `if ASPECT_RATIOS.contains(&ar.as_str())` (`:232`) and the same for `style` (`:239`).

**Why it matters.** The public validators look like the allow-list gate but are not on the gate.
The duplicated inline check is what actually decides, and it is a `contains` against the same
constant — correct today, but two copies of one rule.

**Minimal fix (CUT).** Delete `is_valid_aspect_ratio` / `is_valid_image_size` and have
`determine_aspect_ratio` call the exported validator (or the reverse) so there is one rule.

### SW-S2-GEN-PROVIDERS-5 — `google_imagen::IMAGE_SIZES` is transitively dead

**Producer.** `src/generation/providers/google_imagen.rs:67-68` — `pub const IMAGE_SIZES: &[&str] = &["1K","2K"]`.

**Consumer.** None. `rg -n --glob '*.rs' '\bIMAGE_SIZES\b' .` returns two hits: the definition and
`google_imagen.rs:609`, which is the body of the dead `is_valid_image_size` (SW-S2-GEN-PROVIDERS-4).
Its own test (`google_imagen.rs:805-809`) exercises it only through that dead function.

**What is missing.** A production reader. The provider never emits an image *size* field to the
Gemini API — `build_request_body` (`google_imagen.rs:180-222`) sends `aspectRatio` and a
`sampleCount`, but no `imageSize` — so the constant describes a capability the wire does not have.

**Minimal fix (CUT).** Delete the constant together with `is_valid_image_size`
(SW-S2-GEN-PROVIDERS-4), or CONNECT it to the request body if `imageSize` is meant to be sent.

### SW-S2-GEN-PROVIDERS-6 — `model_aliases` is honoured by 1 of 20 factory arms; `resolve_model` has no production caller

**Producer.** `src/config/types/generation/provider.rs:88` — `pub model_aliases: HashMap<String, String>`,
with the resolver `pub fn resolve_model` at `:177-182`.

**Consumer.** `model_aliases` is read in production in exactly one place:
`src/generation/providers/factory.rs:304` (the `replicate` arm, which builds its own alias→version
map). Every other arm uses `config.default_model()` — i.e. `models[0]`
(`provider.rs:171-173`) — and never consults the alias table: `factory.rs:128, 138, 149, 159, 168,
181, 190, 199, 208, 218, 228, 244, 268, 277, 286, 299, 330, 344, 370`.

`GenerationProviderConfig::resolve_model` itself has **no production caller**:
`rg -n --glob '*.rs' '\bresolve_model\b' .` shows it called only at
`src/config/types/generation/mod.rs:196, 200, 205` (inside `#[cfg(test)] mod tests`) and
`tests/provider_config_probe/serde_tests.rs:328-330` (integration test). The other six hits are
`ReplicateProvider::resolve_model` (`src/generation/providers/replicate/provider.rs:99`, used in
production at `:140`) — a different type with the same method name.

**What is missing.** The dispatch arm that routes each factory arm through `resolve_model`
instead of `default_model()`.

**Why it matters.** An operator writes

```toml
[generation.providers.my-image]
provider_type = "openai"
models = ["dall-e-3"]
model_aliases = { fast = "dall-e-3", hd = "dall-e-2" }
```

and the alias table is silently ignored for everything except `replicate`. The public API
`resolve_model` exists, is documented as “resolving aliases”, is exercised by tests, and is
never called by the code that would need it.

**Minimal fix (DECIDE).** Either CONNECT — replace `config.default_model()` with
`config.resolve_model(config.default_model())` in the arms that should honour aliases — or CUT:
narrow the field to the one provider type that uses it and rename it accordingly, so the schema
stops advertising a general capability.

### SW-S2-GEN-PROVIDERS-7 — the factory’s “Supported Provider Types” doc (and `mod.rs`’s “Available Providers”) omit 11 provider types

**Producer.** `src/generation/providers/factory.rs:29-40` documents nine provider groups
(`openai`/`openai_image`/`dalle`, `openai_tts`/`tts`, `openai_compat`, `stability`/`stability_image`/`sdxl`,
`google`/`google_imagen`/`imagen`, `google_veo`/`veo`, `replicate`, `elevenlabs`, `midjourney`/`mj`).
`src/generation/providers/mod.rs:7-16` lists nine provider types.

**Consumer / source of truth.** The match arms at `factory.rs:126-411` — **20** arms covering
**34** aliases — and the catch-all error message at `factory.rs:404-408`, which enumerates all
34 correctly.

**What is missing.** From the doc: `openai_whisper`/`whisper`, `deepgram_stt`/`deepgram`,
`deepgram_tts`, `azure_speech`/`azure_tts`, `suno`, `bfl`/`bfl_flux`/`flux`, `cartesia`,
`minimax_tts`, `volcengine_tts`, `local`, `fal` — 16 aliases, 11 provider types. From
`mod.rs:7-16`: `azure_speech`, `bfl`, `cartesia`, `deepgram_stt`, `deepgram_tts`, `fal`,
`minimax_tts`, `openai_whisper`, `suno`, `volcengine_tts` (10 of the 19 provider modules declared
at `mod.rs:38-57`), plus the `local` BYO voice provider which is not a module here at all.

**Why it matters.** The doc is the first thing a caller reads to learn which `provider_type`
strings are legal; the same file’s error message says something different. A user who reads the
doc does not know `whisper`, `fal` or `local` exist, and the doc is the only place the full set
is described.

**Minimal fix (CUT).** Delete the false list and regenerate it from the match arms — or drop the
hand-written list entirely and point at the catch-all error string, which is already correct.

### SW-S2-GEN-PROVIDERS-8 — `config.color` reaches 3 of 20 arms, and `GenerationProvider::color()` has no production reader at all

**Producer.** `GenerationProviderConfig::color` is applied by the factory in exactly three arms:
`factory.rs:248` (`openai_compat`), `factory.rs:348` (`fal`), `factory.rs:385` (`midjourney`).

**Consumer.** The other 17 arms ignore it and return a hard-coded colour from the trait method:
`google_imagen.rs:589` (`"#4285F4"`), `openai_image.rs:530` (`"#10a37f"`),
`stability.rs:542` (`"#8b5cf6"`), and the equivalent in the other provider modules.

The trait method itself has **no production reader**: `rg -n --glob '*.rs' '\.color\(\)' .` returns
six hits, and all of them are `AiProvider::color` — a different trait
(`src/providers/metering.rs:423`, `src/providers/model_override_provider.rs:78`,
`src/providers/think_level_provider.rs:83`, `interfaces/tui/src/tui/markdown.rs:471`). The Panel’s
provider chip colour comes from the **config** wire, not from the provider object
(`src/gateway/handlers/generation_providers/wire.rs:96` and `:123`;
`interfaces/webchat/src/platform/wide/views/settings/generation_providers/mod.rs:355`).

**What is missing.** Either a consumer for `GenerationProvider::color()`, or a decision to drop it.
As it stands the provider-side colour is a dead value on every provider in the slice.

**Why it matters.** The observable symptom is nil today (the UI uses the config), but the wiring
is misleading in two directions: an operator who sets `color` on a `stability` provider sees the
Panel chip change and the provider object ignore it, and a maintainer reading `fn color()` in
`google_imagen.rs` reasonably assumes something reads it.

**Minimal fix (DECIDE).** Either CONNECT — apply `config.color` in every arm and surface
`provider.color()` to the UI instead of the raw config — or CUT the trait method and the
per-provider colour constants.

### SW-S2-GEN-PROVIDERS-9 — `config.edit_url` is accepted and echoed for every provider but honoured only by `openai_compat`

**Producer.** `src/config/types/generation/provider.rs:96` — `pub edit_url: Option<String>`.
It is part of the generic provider-config wire: read at
`src/gateway/handlers/generation_providers/mod.rs:98` and written back at `:153`, mirrored in
`wire.rs:105` and `:133`. The Panel shows an “Edit URL” field for **every** custom provider
(`interfaces/webchat/src/platform/wide/views/settings/generation_providers/add_custom.rs:250-251`,
`detail_view.rs:381-382`).

**Consumer.** `src/generation/providers/factory.rs:253-255` — the `openai_compat` arm, and only
that arm. `rg -n --glob '*.rs' 'edit_url' src/` shows no other factory arm reads it.

**What is missing.** The connection for the other image providers.

**Why it matters.** An operator adds a custom provider of type `openai`/`dalle`/`stability`,
fills in the Edit URL the form offered, and the value is persisted, echoed back by the API and
used for nothing — and the provider cannot edit images anyway
(`supports_image_editing()` is the trait default `false`, `src/generation/mod.rs:342`;
see SW-S2-GEN-PROVIDERS-3). The knob is a no-op presented as a feature.

**Minimal fix (DECIDE).** Hide the field unless `provider_type == "openai_compat"`, or CONNECT
image editing for the remaining image providers so the knob has a real far end.

---

## Section B — Clean (verified no wire)

Exported symbols checked in this slice and found properly wired (producer → production consumer):

- `create_provider` — `src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:75` (boot) and `:131` (hot reload); `src/generation/providers/factory.rs:79`
- `factory.rs` “local” arm → `LocalVoiceProvider::from_config` — `factory.rs:89-104`, `src/gateway/voice/local_provider.rs:149`; the literal `"local"` agrees with `LOCAL_PROVIDER_TYPE` (`src/config/types/voice_local.rs:295`), and `generation_init.rs:22` exempts it from the api-key gate exactly as the factory does
- `factory.rs` api-key gate (missing key `:111`, blank key `:117-120`) — `generation_init.rs:17-45` (`hydrate_key_and_gate`), so the gate and the boot-time gate cannot drift
- `factory.rs` end-of-match capability check `provider.supports(gen_type)` — `factory.rs:409-414`; it makes a typed-vs-typed mismatch (e.g. `google_veo` under `[generation.image_providers.*]`) fail closed with `unsupported_generation_type`
- Every factory arm ↔ its constructor (20 arms, 20 constructors) — `factory.rs:126-411` ↔ `openai_image.rs:138`, `openai_tts/mod.rs:104`, `openai_whisper/mod.rs`, `deepgram_stt/mod.rs`, `deepgram_tts/mod.rs`, `azure_speech/mod.rs`, `suno/mod.rs`, `bfl/mod.rs`, `cartesia/mod.rs`, `minimax_tts/mod.rs`, `volcengine_tts/mod.rs`, `openai_compat/builder.rs:160`, `stability.rs:167`, `google_imagen.rs`, `google_veo/`, `replicate/`, `elevenlabs/mod.rs`, `midjourney/`, `fal/mod.rs`
- Every `provider_type` used by a built-in preset has a factory arm — `src/config/types/generation/presets/registry.rs` (`openai`, `google`, `bfl`, `google_veo`, `fal`, `suno`, `openai_tts`, `azure_speech`, `cartesia`, `deepgram_stt`, `deepgram_tts`, `elevenlabs`, `minimax_tts`, `openai_compat`, `openai_whisper`, `replicate`, `stability`, `volcengine_tts`)
- `ResolvedUrl::primary_endpoint` — `url_normalize.rs:22`; readers `openai_tts/mod.rs:156`, `openai_whisper/mod.rs:109`, `openai_image.rs:150`, `openai_compat/builder.rs:186`, `src/gateway/voice/inbound/provider.rs:128`, `src/generation/probe.rs:74-76`
- `resolve_base_url` — `url_normalize.rs:57`; reader `factory.rs:82` (and `url_normalize.rs:63` internally)
- `http::generation_http_client` — `http.rs:42`; readers `bfl/mod.rs:92`, `openai_image.rs:142`, `google_imagen.rs:158`, `stability.rs:172`
- `http::voice_http_client` — `http.rs:26`; readers `openai_tts/mod.rs:149,175`, `azure_speech/mod.rs:113,132`, `src/gateway/voice/local_provider.rs:78`
- `http::WithRequestTimeout::with_timeout` — `http.rs:79`; applied by all 20 factory arms (`factory.rs:126-411`), enforced by the source-scanning test `factory.rs:427-533` (`every_provider_arm_applies_the_timeout_knob`)
- `http.rs:53-58` doc claim (“OpenAiTtsProvider and AzureSpeechProvider deliberately do NOT use this”) — **true**: inherent `pub fn with_timeout` at `openai_tts/mod.rs:171` and `azure_speech/mod.rs:128` shadows the trait method and rebuilds through `voice_http_client`
- `http::retry_transient` — `http.rs:99`; sole production reader `openai_image.rs:437` (429/5xx/timeout/network retry ladder)
- `stability::is_valid_style_preset` — `stability.rs:307`; used in production at `stability.rs:236` (`build_request_body`)
- `stability::STYLE_PRESETS` — `stability.rs:67`; read at `stability.rs:310`
- `google_imagen::ASPECT_RATIOS` — `google_imagen.rs:65`; read at `:232` and `:239` (production) and `:603` (the dead validator)
- `google_imagen::PERSON_GENERATION_OPTIONS` — `google_imagen.rs:71`; read at `:207`
- `openai_image::generations_url` — `openai_image.rs:162`; read at `openai_image.rs:175` (the real POST target)
- `stability::text_to_image_url` — `stability.rs:189-196`; read at `stability.rs:399`; the path `{endpoint}/v1/generation/{model}/text-to-image` matches the documented Stability v1 API
- `google_imagen` predict endpoint — `google_imagen.rs:173-175`; read at `:196`; `{endpoint}/v1beta/models/{model}:predict` matches the documented Gemini API
- `openai_compat::edits_url` (a **different** type from SW-S2-GEN-PROVIDERS-3) — `openai_compat/helpers.rs:44`; read at `openai_compat/edit.rs:198`
- `GenerationProviderConfig::request_timeout_secs` — `provider.rs:149`; read by all 20 factory arms and by `src/gateway/voice/local_provider.rs:44,157` (so the `local` arm honours `timeout_seconds` too)
- `GenerationProviderConfig::default_model` — `provider.rs:171`; read at `factory.rs:128…370` (19 arms) and `src/gateway/voice/local_provider.rs:154`
- `config.defaults.voice` — `defaults.rs:71`; read at `factory.rs:139, 209, 219, 229, 331`
- `config.defaults.format` — `defaults.rs:56`; read at `src/gateway/voice/local_provider.rs:156`
- `config.timeout_seconds` → `request_timeout_secs` → every arm — `provider.rs:149-153`, `factory.rs:126-411`
- `config.capabilities` → `openai_compat` `supported_types` — `factory.rs:256-259`; and → `merged_providers` gen_type — `src/config/types/generation/config.rs:213-220`
- `config.verified` — `provider.rs:92`; read/written by the Panel gateway (`src/gateway/handlers/generation_providers/handlers.rs:299, 482, 737`) and surfaced at `wire.rs:104,132`
- `providers::tests.rs` production symbols — all assertions in `src/generation/providers/tests.rs` exercise `create_provider` (the production entry point), so the tests are **not** the only consumer of anything they touch

---

## Section C — New observations

Adjacent to the findings above; not classified as severed wires.

- `[possible] [low]` `src/generation/providers/factory.rs:89` compares `config.provider_type == "local"`
  with a string literal while the canonical constant is `LOCAL_PROVIDER_TYPE`
  (`src/config/types/voice_local.rs:295`). They agree today; a rename of the constant would
  silently un-exempt the local provider from the api-key gate.
- `[possible] [low]` `is_valid_aspect_ratio` exists twice with different value sets —
  `google_imagen.rs:602` (`["1:1","3:4","4:3","9:16","16:9"]`) and `google_veo/helpers.rs:11`
  (`["16:9","9:16"]`). Same name, different semantics; a future import that picks the wrong one
  would silently change an allow-list.
- `[possible] [medium]` The `base_url` parameter of `OpenAiImageProvider::new` (`openai_image.rs:140`),
  `OpenAiTtsProvider::new` (`openai_tts/mod.rs:107`) and `OpenAiWhisperProvider::new` is
  **redundant in production**: the factory always supplies both `config.base_url.clone()` and the
  pre-resolved `resolved_url` derived from the same string (`factory.rs:82, 126-127, 135-136,
  146-147`), and the constructor prefers `resolved_url`. The `base_url` branch inside the
  constructor is therefore only reachable from tests. Not a severed wire (the parameter is
  honoured), but a two-sources-of-truth hazard.
- `[possible] [low]` `ResolvedUrl::Custom` returns `None` from `secondary_endpoint`
  (`url_normalize.rs:44`), so any provider configured with a full custom URL has **no** edit
  endpoint — even when that custom URL is the images endpoint. Only relevant if image editing is
  ever connected (see SW-S2-GEN-PROVIDERS-3).
- `[possible] [medium]` `GenerationParams` fields `n`, `aspect_ratio`, `seed`, `negative_prompt`,
  `guidance_scale`, `steps` and `format` are read by this slice’s providers
  (`openai_image.rs:213`, `google_imagen.rs:187,231`, `stability.rs:205,222,225,231,241`) but the
  **only** production producer of a `GenerationRequest` in the whole repository is the builtin tool
  (`src/builtin_tools/generation/image_generate.rs:138-146`), whose args struct
  (`ImageGenerateArgs`, `:23-46`) has no such fields. Those provider branches are therefore
  always at their defaults in production. The missing arm is on the tool side (outside this
  slice), so it is reported here rather than as a finding.
- `[possible] [low]` `src/generation/providers/tests.rs` has no factory-arm test for `local`,
  `openai_whisper`/`whisper`, `deepgram_stt`/`deepgram`, `minimax_tts`, `volcengine_tts` or
  `fal` (test functions listed at `tests.rs:10-725`).
- `[possible] [low]` `google_imagen.rs:558` handles “additional images (if sample_count > 1)”, but
  `sample_count` can only exceed 1 if `params.n` is set, which no production producer does
  (previous item). The multi-image branch is unreachable in production.
- `[possible] [low]` `factory.rs` rejects duplicate provider names at registration
  (`src/generation/registry.rs:107-113` → `DuplicateProvider`), and
  `generation_init.rs:76` discards that error with `.is_ok()`, so a legacy
  `[generation.providers.X]` entry with two capabilities silently keeps only the first
  registration — no warning is logged for the dropped one.
- `[possible] [low]` `http.rs`’s doc names two providers that skip `WithRequestTimeout`
  (`http.rs:53-58`); a third `voice_http_client` user exists —
  `src/gateway/voice/local_provider.rs:78` — which the doc does not mention (it builds its own
  client from `request_timeout_secs()`, so the behaviour is correct; only the prose is incomplete).

---

## Section D — Coverage statement

**Slice files read in full (before any judgement):**
`src/generation/providers/mod.rs` (81), `factory.rs` (533), `http.rs` (187),
`url_normalize.rs` (202), `google_imagen.rs` (975), `openai_image.rs` (959),
`stability.rs` (1012), `tests.rs` (725). Production/test boundaries were located by the
`#[cfg(test)] mod tests` markers at `google_imagen.rs:612`, `openai_image.rs:539`,
`stability.rs:551`, `factory.rs:427`, `http.rs:137`, `url_normalize.rs:90`, `mod.rs:59`.

**Lenses applied per file**

- `factory.rs` — L1 registration parity (20 arms vs 20 constructors vs 34 aliases in the
  catch-all), L2 call-vs-handler parity (every constructor call), L3 classifier-vs-handler
  (`provider.supports(gen_type)` at `:409-414`; `capabilities` → `supported_types`),
  L5 config-reader parity (`defaults.voice`, `color`, `edit_url`, `model_aliases`, `timeout_seconds`,
  `base_url`, `capabilities` — every field of `GenerationProviderConfig` that the factory reads),
  L7 stub sweep (every arm returns a real provider; the api-key and blank-key gates).
- `url_normalize.rs` — L6 path/route parity (`/v1/images/generations`, `/v1/videos/generations`,
  `/v1/audio/speech`, `/v1/audio/generations`, `/v1/audio/transcriptions`, `/v1/images/edits`
  against the endpoints the providers actually POST to), L5 (`ResolvedUrl` readers),
  L7 (`secondary_endpoint` — one reader, itself dead → SW-S2-GEN-PROVIDERS-3).
- `http.rs` — L2 (`voice_http_client` / `generation_http_client` / `WithRequestTimeout` /
  `retry_transient` consumers), L7 (the doc claim about the two providers that skip
  `WithRequestTimeout` — verified true), L3 (timeout policy vs the providers that build their own
  client: `openai_tts/mod.rs:149,175`, `azure_speech/mod.rs:113,132`,
  `src/gateway/voice/local_provider.rs:78`).
- `google_imagen.rs` — L1/L2 (allow-lists `ASPECT_RATIOS`, `IMAGE_SIZES`, `PERSON_GENERATION_OPTIONS`
  vs their readers), L6 (Gemini `predict` endpoint), L7 (`generate()` performs a real POST —
  `google_imagen.rs:196-270`), L5 (`request.params.*` readers vs the tool that produces them).
- `openai_image.rs` — L1/L2 (`edits_url` — SW-S2-GEN-PROVIDERS-3), L6 (`/v1/images/generations`
  and the size allow-list at `:189-192`), L7 (`generate()` performs a real POST; retry ladder at
  `:437`), L5 (`params.n`/`quality`/`style` readers).
- `stability.rs` — L1/L2 (`STYLE_PRESETS`, `is_valid_style_preset`), L6
  (`{endpoint}/v1/generation/{model}/text-to-image`), L7 (`generate()` performs a real POST at
  `:404-411`), L5 (`params.*` readers at `:196-248`).
- `mod.rs` — L1 registration parity (the 19 provider modules vs the 9 documented), L2
  (`create_provider` re-export and its production callers).
- `tests.rs` — classified as test-only throughout; used to determine which production symbols have
  **no** production consumer (SW-S2-GEN-PROVIDERS-1, -3, -4, -5, -6) and to list the factory arms
  without a test (Section C).

**Lenses that did not apply to this slice:** L4 (event emit-vs-subscribe) — no bus events are
emitted or subscribed in the slice; no `GlobalBus`/event-bus usage exists in any slice file
(checked with `rg` over `src/generation/providers/`).

**Seams not covered**

- The `openai_compat` module itself (`builder.rs`, `generate.rs`, `helpers.rs`, `edit.rs`,
  `provider.rs`, `types.rs`) is **not** a slice file; it was read only to establish the far end of
  SW-S2-GEN-PROVIDERS-2/-3 and the `edit_url` arm. Its own internal wires were not swept.
- The 17 provider modules outside this slice (`suno`, `bfl`, `fal`, `cartesia`, `minimax_tts`,
  `volcengine_tts`, `deepgram_stt`, `deepgram_tts`, `azure_speech`, `elevenlabs`, `midjourney`,
  `replicate`, `google_veo`, `openai_tts`, `openai_whisper`) were read only at the points where the
  factory calls them. Their internal wires, endpoint formats and config readers are other slices’
  responsibility.
- The Panel side (`interfaces/webchat/`) was read only where a config field’s UI surface was
  needed to judge whether a knob is inert; the Panel’s own request/response contract was not swept.
- `src/config/types/generation/presets/registry.rs` was used to enumerate the `provider_type`
  values that must have a factory arm; the presets’ `base_url`/`model`/`capabilities` payloads
  were checked for endpoint correctness only for the openai_compat entries.
- No build, test or clippy run was performed (forbidden by the brief); all verdicts rest on the
  source as read plus whole-repository `rg`.

**Counts:** 9 findings — 1 high, 6 medium, 2 low.
