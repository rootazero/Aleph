//! `OpenAI` strict schema normalization.
//!
//! Recursively injects `additionalProperties: false` and ensures `properties`
//! exists on all `type: "object"` nodes, matching `OpenAI`'s strict mode
//! requirements.

use serde_json::Value;

/// Outcome of normalizing a JSON schema for `OpenAI` strict mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrictResult {
    /// Schema is fully normalized and strict-compatible.
    Ok,
    /// Schema contains a sub-tree that cannot be expressed in strict mode.
    /// Caller should downgrade the affected tool to `strict: None`.
    Incompatible {
        /// Diagnostic for `tracing::warn!`. Format:
        /// `"<json-pointer>: <description>"` e.g.
        /// `".properties.actions.items: multi-type schema is not strict-compatible (types: [boolean,object,array,number,string,integer,null])"`.
        reason: String,
    },
}

/// Sanitize the top-level envelope of a tool parameters schema for `OpenAI`'s
/// tool schema validator.
///
/// `OpenAI`'s parser rejects top-level schemas that:
/// - lack `type: "object"` (`schema must be a JSON Schema of 'type: "object"', got 'type: "None"'`)
/// - contain top-level `oneOf` / `anyOf` / `allOf` / `enum` / `not`
///   (`schema must have type 'object' and not have 'oneOf'/'anyOf'/'allOf'/'enum'/'not' at the top level`)
///
/// This function rewrites the root in-place:
/// 1. Injects `type: "object"` if absent.
/// 2. Flattens top-level `oneOf` / `anyOf` branches into root `properties`:
///    each branch's `properties` is merged in (first-branch-wins on key
///    collision, except for a key that *every* declaring branch pins with
///    `const` / `enum` — that widens to the union of the pinned values),
///    `required` becomes the intersection of all branches' required sets, and
///    the union keyword is removed.
/// 3. Strips top-level `allOf` / `enum` / `not` (rare; no flatten attempt).
/// 4. Ensures root `properties: {}` exists even if empty.
///
/// The flatten is permissive rather than lossy on discriminator constraints:
/// the widened `enum` names every legal tag value instead of pinning the first
/// branch's. What it does drop is the per-branch co-occurrence rule ("`content`
/// only accompanies `action: add`") — every field-level type survives, and tool
/// descriptions carry the co-occurrence semantics for LLM guidance.
///
/// Non-recursive (top-level only). Idempotent.
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub fn ensure_openai_tool_envelope(schema: &mut Value) {
    let map = match schema.as_object_mut() {
        Some(m) => m,
        None => return,
    };

    // 1. Inject type:object if absent.
    if !map.contains_key("type") {
        map.insert("type".to_string(), Value::String("object".to_string()));
    }

    // 2. Flatten top-level oneOf / anyOf into root properties.
    let mut required_sets: Vec<std::collections::HashSet<String>> = Vec::new();
    for union_key in ["oneOf", "anyOf"] {
        let branches = match map.remove(union_key) {
            Some(Value::Array(arr)) => arr,
            Some(other) => {
                // Not an array — put back; nothing to flatten.
                map.insert(union_key.to_string(), other);
                continue;
            }
            None => continue,
        };

        let mut merged_props: serde_json::Map<String, Value> = serde_json::Map::new();
        // Per-key discriminator accounting: how many branches declare the key,
        // and every literal value they pin, in branch order. The value slot
        // drops to `None` as soon as one declaring branch leaves the key open —
        // such a key is not a discriminator and keeps first-branch-wins.
        let mut pinned: std::collections::HashMap<String, (usize, Option<Vec<Value>>)> =
            std::collections::HashMap::new();
        required_sets.clear();

        for branch in &branches {
            let branch_obj = match branch.as_object() {
                Some(o) => o,
                None => continue,
            };
            if let Some(Value::Object(props)) = branch_obj.get("properties") {
                for (k, v) in props {
                    // rust-doctor-disable-next-line excessive-clone
                    let slot = pinned.entry(k.clone()).or_insert((0, Some(Vec::new())));
                    slot.0 += 1;
                    match pinned_values(v) {
                        Some(values) => {
                            if let Some(acc) = slot.1.as_mut() {
                                acc.extend(values);
                            }
                        }
                        None => slot.1 = None,
                    }
                    // rust-doctor-disable-next-line excessive-clone
                    merged_props.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            if let Some(Value::Array(req)) = branch_obj.get("required") {
                let set: std::collections::HashSet<String> = req
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                required_sets.push(set);
            }
        }

        // Widen colliding discriminators before the merge. schemars renders an
        // internally tagged enum (`#[serde(tag = "action")]`) as one branch per
        // variant, each pinning the tag with `const`. First-branch-wins would
        // ship `{"const": "<first variant>"}` — the schema would tell the model
        // that the first action is the ONLY legal one while every other
        // variant's fields sit beside it as siblings, contradicting both the
        // tool description and the other properties it just merged in.
        for (key, (declaring, values)) in &pinned {
            // `< 2` is not a collision: a key only one branch declares keeps
            // its own `const`, which is still exactly true.
            if *declaring < 2 {
                continue;
            }
            let Some(values) = values else { continue };
            let mut deduped: Vec<Value> = Vec::with_capacity(values.len());
            for v in values {
                if !deduped.contains(v) {
                    // rust-doctor-disable-next-line excessive-clone
                    deduped.push(v.clone());
                }
            }
            // Tags are strings in practice; ask the shared inference rather
            // than hardcode it so a numeric `const` union does not ship a
            // self-contradicting `{"type":"string","enum":[1,2]}`.
            let ty = infer_type_from_values(&deduped);
            // rust-doctor-disable-next-line excessive-clone
            merged_props.insert(key.clone(), serde_json::json!({"type": ty, "enum": deduped}));
        }

        // Merge flattened properties into root properties (first-branch wins).
        if !merged_props.is_empty() {
            let root_props = map
                .entry("properties".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(p) = root_props {
                for (k, v) in merged_props {
                    p.entry(k).or_insert(v);
                }
            }
        }

        // Intersection of `required` across all branches.
        if !required_sets.is_empty() {
            let mut iter = required_sets.drain(..);
            let mut intersection = iter.next().unwrap_or_default();
            for set in iter {
                intersection = intersection.intersection(&set).cloned().collect();
            }
            if !intersection.is_empty() {
                let root_req = map
                    .entry("required".to_string())
                    // rust-doctor-disable-next-line unnecessary-allocation
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(arr) = root_req {
                    let existing: std::collections::HashSet<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    for r in intersection {
                        if !existing.contains(&r) {
                            arr.push(Value::String(r));
                        }
                    }
                }
            }
        }
    }

    // 3. Strip top-level allOf / enum / not (rare; no flatten attempt).
    map.remove("allOf");
    map.remove("enum");
    map.remove("not");

    // 4. Ensure root properties exists.
    if !map.contains_key("properties") {
        map.insert(
            "properties".to_string(),
            Value::Object(serde_json::Map::new()),
        );
    }
}

/// The literal values a property schema pins, if it pins any: `{"const": X}`
/// → `[X]`, `{"enum": [...]}` → that list. `None` when the schema leaves the
/// value open, which makes the key non-enumerable for the union flatten in
/// [`ensure_openai_tool_envelope`].
fn pinned_values(schema: &Value) -> Option<Vec<Value>> {
    let obj = schema.as_object()?;
    if let Some(constant) = obj.get("const") {
        // rust-doctor-disable-next-line excessive-clone
        return Some(vec![constant.clone()]);
    }
    match obj.get("enum") {
        // rust-doctor-disable-next-line excessive-clone
        Some(Value::Array(values)) if !values.is_empty() => Some(values.clone()),
        _ => None,
    }
}

/// Provider-agnostic name for [`ensure_openai_tool_envelope`].
///
/// The flatten is only *documented* against `OpenAI`'s validator; in substance
/// it is what any provider needs when it rejects top-level `oneOf` / `anyOf`.
/// Without it a schemars tagged-enum tool ships with an empty `properties` and
/// the model has to guess argument names out of the description prose.
/// Anthropic is the second caller (`protocols::anthropic::adapter`, which runs
/// this first and keeps its own strip as the backstop). The alias exists so
/// that call site does not read as "Anthropic borrows an `OpenAI` quirk" —
/// the implementation above stays the only one, because a second copy would
/// drift the moment either provider's envelope rules move.
///
/// Note for non-`OpenAI` callers: this also drops top-level `enum` / `not`,
/// which are meaningless on a tool parameter object.
pub(crate) fn flatten_tool_schema_unions(schema: &mut Value) {
    ensure_openai_tool_envelope(schema);
}

/// Lenient multi-type rewriter for `OpenAI`'s non-strict tool path.
///
/// `OpenAI`'s tool schema validator rejects `type: [...]` arrays even in
/// non-strict mode. This function rewrites them in-place:
/// - `["null", X]` (length 2, one null) → `{"anyOf": [{"type":"null"}, {..., "type": X}]}`
///   preserves the schema lossless-ly.
/// - Any other multi-type shape → strip the `type` keyword.
///   The field becomes any-value (lossy but accepted).
///
/// Idempotent. Use in the non-strict `build_tools` path where the
/// stricter `normalize_strict_schema` returns Incompatible.
pub fn lenient_multi_type_rewrite(schema: &mut Value) {
    lenient_rewrite_node(schema);
}

fn lenient_rewrite_node(node: &mut Value) {
    if let Value::Object(map) = node {
        let type_array = map.get("type").and_then(|v| v.as_array()).cloned();
        if let Some(types) = type_array {
            if types.len() == 2 {
                let null_idx = types.iter().position(|t| t.as_str() == Some("null"));
                let other_idx = types
                    .iter()
                    .position(|t| t.as_str().is_some_and(|s| s != "null"));
                if let (Some(_), Some(other)) = (null_idx, other_idx) {
                    // rust-doctor-disable-next-line excessive-clone
                    let other_type = types[other].clone();
                    let mut non_null_branch: serde_json::Map<String, Value> = map
                        .iter()
                        .filter(|(k, _)| k.as_str() != "type")
                        // rust-doctor-disable-next-line excessive-clone
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    non_null_branch.insert("type".to_string(), other_type);
                    map.clear();
                    map.insert(
                        "anyOf".to_string(),
                        Value::Array(vec![
                            serde_json::json!({"type": "null"}),
                            Value::Object(non_null_branch),
                        ]),
                    );
                    if let Some(Value::Array(arr)) = map.get_mut("anyOf") {
                        if let Some(non_null) = arr.get_mut(1) {
                            lenient_rewrite_node(non_null);
                        }
                    }
                    return; // map is now {"anyOf": [...]}, no other keys to recurse
                }
            }
            // Any other multi-type → strip type
            map.remove("type");
        }

        if let Some(Value::Object(props)) = map.get_mut("properties") {
            for (_, prop_schema) in props.iter_mut() {
                lenient_rewrite_node(prop_schema);
            }
        }
        if let Some(items) = map.get_mut("items") {
            lenient_rewrite_node(items);
        }
        for key in &["anyOf", "allOf", "oneOf"] {
            if let Some(Value::Array(variants)) = map.get_mut(*key) {
                for variant in variants.iter_mut() {
                    lenient_rewrite_node(variant);
                }
            }
        }
    }
}

/// Recursively normalize a JSON schema for `OpenAI` strict mode.
///
/// - Injects `additionalProperties: false` on all object schemas
/// - Ensures `properties` exists (empty object if missing)
/// - Recursively descends into `properties`, `items`, `anyOf`, `allOf`, `oneOf`
/// - Optionally sets top-level `strict: true`
/// - Rewrites `type: ["null", X]` (length 2 with one null) to
///   `{"anyOf": [{"type":"null"}, {<sibling keys>, "type": X}]}`
/// - Any other multi-type shape returns `Incompatible` with a
///   JSON-pointer-prefixed reason (caller should downgrade `strict`)
/// - Returns `Incompatible` when it encounters a `$ref` node or a `$defs` /
///   `definitions` bucket: references are never resolved here, so nested
///   subschemas behind them would ship un-normalized (e.g. objects missing
///   `additionalProperties: false`), which `OpenAI`'s strict validator rejects
///   for the whole request. Callers that want strict for such schemas must
///   inline references first (see [`deref_json_schema`])
/// - Returns `StrictResult::Ok` on success, `StrictResult::Incompatible { reason }`
///   when a sub-schema can't be expressed in strict mode
pub fn normalize_strict_schema(schema: &mut Value, set_top_level_strict: bool) -> StrictResult {
    normalize_node(schema, set_top_level_strict, true, "")
}

// rust-doctor-disable-next-line high-cyclomatic-complexity
fn normalize_node(
    node: &mut Value,
    set_strict: bool,
    is_top_level: bool,
    path: &str,
) -> StrictResult {
    if let Value::Object(map) = node {
        // This normalizer never resolves `$ref` targets and never descends
        // into `$defs` / `definitions` buckets, yet OpenAI's strict validator
        // checks every nested subschema reachable through them (e.g. tagged
        // enums like RememberArgs::Batch, where schemars emits
        // `operations.items.$ref` → `#/$defs/SingleOp`). Shipping such a
        // schema with `strict: true` 400s the whole request, so bail out and
        // let the caller downgrade this tool to the non-strict path.
        if let Some(ref_target) = map.get("$ref") {
            let target = ref_target.as_str().unwrap_or("<non-string>");
            return StrictResult::Incompatible {
                reason: format!(
                    "{path}: unresolved $ref is not strict-compatible (target: {target})"
                ),
            };
        }
        for bucket in ["$defs", "definitions"] {
            if map.contains_key(bucket) {
                return StrictResult::Incompatible {
                    reason: format!(
                        "{path}: {bucket} bucket is not strict-compatible \
                         (nested subschemas are not normalized)"
                    ),
                };
            }
        }

        if is_top_level && set_strict {
            map.insert("strict".to_string(), Value::Bool(true));
        }

        let type_array = map.get("type").and_then(|v| v.as_array()).cloned();
        if let Some(types) = type_array {
            // Case 1: exactly ["null", X] with one non-null type → anyOf transform
            if types.len() == 2 {
                let null_idx = types.iter().position(|t| t.as_str() == Some("null"));
                let other_idx = types
                    .iter()
                    .position(|t| t.as_str().is_some_and(|s| s != "null"));
                if let (Some(_), Some(other)) = (null_idx, other_idx) {
                    // rust-doctor-disable-next-line excessive-clone
                    let other_type = types[other].clone();
                    let mut non_null_branch: serde_json::Map<String, Value> = map
                        .iter()
                        .filter(|(k, _)| k.as_str() != "type")
                        // rust-doctor-disable-next-line excessive-clone
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    non_null_branch.insert("type".to_string(), other_type);
                    map.clear();
                    map.insert(
                        "anyOf".to_string(),
                        Value::Array(vec![
                            serde_json::json!({"type": "null"}),
                            Value::Object(non_null_branch),
                        ]),
                    );
                    if let Some(Value::Array(arr)) = map.get_mut("anyOf") {
                        if let Some(non_null) = arr.get_mut(1) {
                            let result = normalize_node(non_null, false, false, path);
                            if matches!(result, StrictResult::Incompatible { .. }) {
                                return result;
                            }
                        }
                    }
                    return StrictResult::Ok;
                }
            }
            // Case 2: anything else (multi non-null, or length != 2) → bail
            let type_list = types
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<&str>>()
                .join(",");
            return StrictResult::Incompatible {
                reason: format!(
                    "{path}: multi-type schema is not strict-compatible (types: [{type_list}])"
                ),
            };
        }

        let is_object = map.get("type").and_then(|t| t.as_str()) == Some("object");

        if is_object {
            if !map.contains_key("properties") {
                map.insert("properties".to_string(), Value::Object(Default::default()));
            }

            map.insert("additionalProperties".to_string(), Value::Bool(false));

            if let Some(Value::Object(props)) = map.get_mut("properties") {
                for (k, prop_schema) in props.iter_mut() {
                    let child_path = format!("{path}.properties.{k}");
                    let result = normalize_node(prop_schema, false, false, &child_path);
                    if matches!(result, StrictResult::Incompatible { .. }) {
                        return result;
                    }
                }
            }
        }

        if let Some(items) = map.get_mut("items") {
            let child_path = format!("{path}.items");
            let result = normalize_node(items, false, false, &child_path);
            if matches!(result, StrictResult::Incompatible { .. }) {
                return result;
            }
        }

        for key in &["anyOf", "allOf", "oneOf"] {
            if let Some(Value::Array(variants)) = map.get_mut(*key) {
                for (idx, variant) in variants.iter_mut().enumerate() {
                    let child_path = format!("{path}.{key}[{idx}]");
                    let result = normalize_node(variant, false, false, &child_path);
                    if matches!(result, StrictResult::Incompatible { .. }) {
                        return result;
                    }
                }
            }
        }
    }
    StrictResult::Ok
}

// =============================================================================
// Moonshot (Kimi) compatibility helpers
// =============================================================================

/// Recursively inline local `$ref` entries in a JSON Schema.
///
/// Moonshot's tool-schema validator rejects (or misinterprets) `$ref` nodes,
/// reporting errors such as
/// `"At path 'properties.action': detected infinite recursion without termination condition"`.
/// This function resolves every local `#/$defs/<Name>` / `#/definitions/<Name>`
/// reference and merges the referenced payload in-place. Definition buckets are
/// removed afterwards so the emitted schema contains no `$ref`s.
///
/// Remote references (e.g. `http://...`) are left untouched. Unresolvable
/// local `$ref`s are left in place rather than panicking.
pub fn deref_json_schema(schema: &mut Value) {
    // Collect definitions first so we can resolve them while mutating the tree.
    let defs = collect_defs(schema);
    // Recursion uses a visited set to guard against true cycles. In practice
    // Aleph tool schemas are acyclic; this guard exists for defensiveness.
    let mut visiting = std::collections::HashSet::new();
    deref_node(schema, &defs, &mut visiting);
    // Remove the now-unneeded definition buckets.
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$defs");
        obj.remove("definitions");
    }
}

fn collect_defs(schema: &Value) -> std::collections::HashMap<String, Value> {
    let mut out = std::collections::HashMap::new();
    for key in ["$defs", "definitions"] {
        if let Some(Value::Object(map)) = schema.get(key) {
            for (k, v) in map {
                // rust-doctor-disable-next-line excessive-clone
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

fn deref_node(
    node: &mut Value,
    defs: &std::collections::HashMap<String, Value>,
    visiting: &mut std::collections::HashSet<String>,
) {
    if let Some(obj) = node.as_object_mut() {
        if let Some(Value::String(ref_path)) = obj.get("$ref") {
            if let Some(name) = ref_path
                .strip_prefix("#/$defs/")
                .or_else(|| ref_path.strip_prefix("#/definitions/"))
            {
                if let Some(def) = defs.get(name) {
                    if visiting.insert(name.to_string()) {
                        // rust-doctor-disable-next-line excessive-clone
                        let mut target = def.clone();
                        deref_node(&mut target, defs, visiting);
                        visiting.remove(name);
                        // Replace the $ref object with the dereferenced target.
                        *node = target;
                    }
                    // If the ref is already being visited we have a real cycle.
                    // Leave the $ref in place; higher-level tooling should reject
                    // truly recursive tool schemas rather than silently inlining
                    // forever. In practice Aleph tool schemas are acyclic.
                }
                // If the reference cannot be resolved, leave the $ref in place.
            }
        }
    }

    // Recurse into the (possibly replaced) node.
    match node {
        Value::Object(obj) => {
            for v in obj.values_mut() {
                deref_node(v, defs, visiting);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                deref_node(v, defs, visiting);
            }
        }
        _ => {}
    }
}

/// Ensure every property schema carries an explicit `type` keyword.
///
/// Moonshot rejects tool parameter schemas where a nested property omits
/// `type` (e.g. `{"enum": ["a", "b"]}` without `"type": "string"`),
/// returning HTTP 400 with `"At path 'properties.X': type is not defined"`.
/// This function walks property-schema positions and fills in a `type` when
/// one is missing:
///
/// - `enum` / `const` present → infer from the values.
/// - structural keywords (`properties`, `items`, `minLength`, …) → infer object/
///   array/string/number.
/// - otherwise default to `"string"`.
///
/// Nodes that legitimately declare shape via combinators (`anyOf`/`oneOf`/
/// `allOf`/`$ref`/`not`/`if`/`then`/`else`) are left alone.
pub fn ensure_property_types(schema: &mut Value) {
    ensure_property_types_node(schema);
}

/// JSON Schema keywords that describe a property's shape without a `type`.
/// When any of these are present we skip the type-filling step.
const COMBINATOR_KEYS: &[&str] = &[
    "anyOf", "oneOf", "allOf", "not", "if", "then", "else", "$ref",
];

/// Structural keywords that imply a specific JSON Schema type.
const OBJECT_KEYWORDS: &[&str] = &[
    "properties",
    "additionalProperties",
    "patternProperties",
    "propertyNames",
    "required",
    "minProperties",
    "maxProperties",
];
const ARRAY_KEYWORDS: &[&str] = &[
    "items",
    "prefixItems",
    "minItems",
    "maxItems",
    "uniqueItems",
    "contains",
];
const STRING_KEYWORDS: &[&str] = &["minLength", "maxLength", "pattern", "format"];
const NUMERIC_KEYWORDS: &[&str] = &[
    "minimum",
    "maximum",
    "multipleOf",
    "exclusiveMinimum",
    "exclusiveMaximum",
];

fn ensure_property_types_node(node: &mut Value) {
    if let Some(obj) = node.as_object_mut() {
        if let Some(Value::Object(props)) = obj.get_mut("properties") {
            for prop in props.values_mut() {
                normalize_property_type(prop);
            }
        }

        if let Some(items) = obj.get_mut("items") {
            if let Some(arr) = items.as_array_mut() {
                for item in arr {
                    normalize_property_type(item);
                }
            } else {
                normalize_property_type(items);
            }
        }

        if let Some(additional) = obj.get_mut("additionalProperties") {
            if additional.is_object() {
                normalize_property_type(additional);
            }
        }

        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(Value::Array(branches)) = obj.get_mut(key) {
                for branch in branches.iter_mut() {
                    normalize_property_type(branch);
                }
            }
        }
    }
}

fn normalize_property_type(node: &mut Value) {
    if let Some(obj) = node.as_object_mut() {
        if !obj.contains_key("type") && !COMBINATOR_KEYS.iter().any(|k| obj.contains_key(*k)) {
            let inferred = if let Some(Value::Array(values)) = obj.get("enum") {
                if !values.is_empty() {
                    infer_type_from_values(values)
                } else {
                    infer_type_from_structure(obj)
                }
            } else if let Some(v) = obj.get("const") {
                infer_type_from_value(v)
            } else {
                infer_type_from_structure(obj)
            };
            obj.insert("type".to_string(), Value::String(inferred));
        }
    }
    ensure_property_types_node(node);
}

fn infer_type_from_structure(obj: &serde_json::Map<String, Value>) -> String {
    if OBJECT_KEYWORDS.iter().any(|k| obj.contains_key(*k)) {
        return "object".to_string();
    }
    if ARRAY_KEYWORDS.iter().any(|k| obj.contains_key(*k)) {
        return "array".to_string();
    }
    if STRING_KEYWORDS.iter().any(|k| obj.contains_key(*k)) {
        return "string".to_string();
    }
    if NUMERIC_KEYWORDS.iter().any(|k| obj.contains_key(*k)) {
        return "number".to_string();
    }
    "string".to_string()
}

fn infer_type_from_value(value: &Value) -> String {
    match value {
        Value::Bool(_) => "boolean".to_string(),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer".to_string()
            } else {
                "number".to_string()
            }
        }
        Value::String(_) => "string".to_string(),
        Value::Null => "null".to_string(),
        Value::Object(_) => "object".to_string(),
        Value::Array(_) => "array".to_string(),
    }
}

fn infer_type_from_values(values: &[Value]) -> String {
    let mut inferred = std::collections::HashSet::new();
    for v in values {
        inferred.insert(infer_type_from_value(v));
    }

    if inferred.len() == 1 {
        if let Some(ty) = inferred.iter().next() {
            // rust-doctor-disable-next-line excessive-clone
            return ty.clone();
        }
    }
    if inferred == std::collections::HashSet::from(["integer".to_string(), "number".to_string()]) {
        return "number".to_string();
    }
    // Mixed values: default to string. Moonshot tolerates enum values that do
    // not exactly match the declared type, so "string" is the safe fallback.
    "string".to_string()
}

/// Mimics the schemars output for `RememberArgs` — an internally tagged enum
/// (`#[serde(tag = "action")]`) whose `Batch` variant carries
/// `operations.items.$ref` → `#/$defs/SingleOp`.
///
/// Shared with the Anthropic adapter tests on purpose: both providers run the
/// same flatten over this shape, so both must be pinned against the same
/// fixture — otherwise a fix on one side silently stops covering the other,
/// which is exactly how this schema came to reach Claude with zero parameters.
#[cfg(test)]
pub(crate) fn remember_args_shaped_schema() -> Value {
    serde_json::json!({
        "oneOf": [
            {"type": "object", "properties": {"action": {"type": "string", "const": "add"}, "content": {"type": "string"}}, "required": ["action", "content"]},
            {"type": "object", "properties": {"action": {"type": "string", "const": "remove"}, "old_text": {"type": "string"}}, "required": ["action", "old_text"]},
            {"type": "object", "properties": {"action": {"type": "string", "const": "batch"}, "operations": {"type": "array", "items": {"$ref": "#/$defs/SingleOp"}}}, "required": ["action", "operations"]}
        ],
        "$defs": {
            "SingleOp": {
                "oneOf": [
                    {"type": "object", "properties": {"action": {"type": "string", "const": "add"}, "content": {"type": "string"}}, "required": ["action", "content"]},
                    {"type": "object", "properties": {"action": {"type": "string", "const": "remove"}, "old_text": {"type": "string"}}, "required": ["action", "old_text"]}
                ]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inject_additional_properties() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });
        assert_eq!(
            normalize_strict_schema(&mut schema, false),
            StrictResult::Ok
        );
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn test_ensure_properties_exists() {
        let mut schema = serde_json::json!({
            "type": "object"
        });
        assert_eq!(
            normalize_strict_schema(&mut schema, false),
            StrictResult::Ok
        );
        assert!(schema["properties"].is_object());
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn test_nested_objects() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "age": { "type": "integer" }
                    }
                }
            }
        });
        assert_eq!(
            normalize_strict_schema(&mut schema, false),
            StrictResult::Ok
        );
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["user"]["additionalProperties"], false);
    }

    #[test]
    fn test_top_level_strict_flag() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        assert_eq!(normalize_strict_schema(&mut schema, true), StrictResult::Ok);
        assert_eq!(schema["strict"], true);
        assert!(!schema["properties"]
            .as_object()
            .unwrap()
            .contains_key("strict"));
    }

    #[test]
    fn test_array_items() {
        let mut schema = serde_json::json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                }
            }
        });
        assert_eq!(
            normalize_strict_schema(&mut schema, false),
            StrictResult::Ok
        );
        assert_eq!(schema["items"]["additionalProperties"], false);
    }

    #[test]
    fn test_any_of_composite() {
        let mut schema = serde_json::json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": { "a": { "type": "string" } }
                },
                {
                    "type": "object",
                    "properties": { "b": { "type": "integer" } }
                }
            ]
        });
        assert_eq!(
            normalize_strict_schema(&mut schema, false),
            StrictResult::Ok
        );
        assert_eq!(schema["anyOf"][0]["additionalProperties"], false);
        assert_eq!(schema["anyOf"][1]["additionalProperties"], false);
    }

    #[test]
    fn test_preserves_required() {
        let mut schema = serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" }
            }
        });
        assert_eq!(
            normalize_strict_schema(&mut schema, false),
            StrictResult::Ok
        );
        let req = schema["required"].as_array().unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0], "name");
    }

    #[test]
    fn test_non_object_unchanged() {
        let mut schema = serde_json::json!({
            "type": "string"
        });
        assert_eq!(
            normalize_strict_schema(&mut schema, false),
            StrictResult::Ok
        );
        assert!(!schema
            .as_object()
            .unwrap()
            .contains_key("additionalProperties"));
    }

    #[test]
    fn multi_type_null_pair_becomes_anyof() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert_eq!(result, StrictResult::Ok);
        let any_of = schema["anyOf"].as_array().expect("anyOf array");
        assert_eq!(any_of.len(), 2);
        assert_eq!(any_of[0]["type"], "null");
        assert_eq!(any_of[1]["type"], "string");
        assert!(schema.get("type").is_none());
    }

    #[test]
    fn null_pair_preserves_sibling_keywords() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"],
            "description": "an optional label",
            "minLength": 1
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert_eq!(result, StrictResult::Ok);
        let non_null_branch = &schema["anyOf"][1];
        assert_eq!(non_null_branch["type"], "string");
        assert_eq!(non_null_branch["description"], "an optional label");
        assert_eq!(non_null_branch["minLength"], 1);
    }

    #[test]
    fn multi_type_any_value_returns_incompatible() {
        let mut schema = serde_json::json!({
            "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(reason.contains("multi-type"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
    }

    #[test]
    fn multi_type_non_null_pair_returns_incompatible() {
        let mut schema = serde_json::json!({
            "type": ["string", "number"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert!(matches!(result, StrictResult::Incompatible { .. }));
    }

    #[test]
    fn ensure_envelope_injects_type_object_for_top_level_oneof() {
        let mut schema = serde_json::json!({
            "oneOf": [
                {"type": "object", "properties": {"action": {"const": "add"}}},
                {"type": "object", "properties": {"action": {"const": "remove"}}}
            ]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(schema["type"], "object");
        assert!(schema.get("oneOf").is_none(), "oneOf flattened + removed");
        assert!(schema["properties"]["action"].is_object());
    }

    #[test]
    fn envelope_skips_when_type_already_present() {
        let mut schema = serde_json::json!({
            "type": "object",
            "oneOf": [{"properties": {"a": {"type":"string"}}, "required":["a"]}]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(schema["type"], "object", "type preserved");
        assert!(schema.get("oneOf").is_none(), "oneOf still flattened");
        assert!(schema["properties"]["a"].is_object());
    }

    #[test]
    fn envelope_injects_type_object_unconditionally_when_absent() {
        let mut schema = serde_json::json!({"properties": {"a": {"type": "string"}}});
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(schema["type"], "object", "always inject when missing");
    }

    #[test]
    fn envelope_flattens_top_level_oneof_into_properties() {
        // Mimic RememberArgs schemars output
        let mut schema = serde_json::json!({
            "oneOf": [
                {"type": "object", "properties": {"action": {"type":"string","const":"add"}, "content": {"type":"string"}}, "required": ["action","content"]},
                {"type": "object", "properties": {"action": {"type":"string","const":"remove"}, "old_text":{"type":"string"}}, "required":["action","old_text"]}
            ]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(schema["type"], "object");
        assert!(schema.get("oneOf").is_none(), "oneOf removed");
        let props = schema["properties"].as_object().expect("properties");
        assert!(props.contains_key("action"));
        assert!(props.contains_key("content"));
        assert!(props.contains_key("old_text"));
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(
            required,
            vec!["action"],
            "intersection of required across branches"
        );
    }

    #[test]
    fn envelope_widens_colliding_discriminator_to_enum() {
        let mut schema = remember_args_shaped_schema();
        ensure_openai_tool_envelope(&mut schema);
        let action = &schema["properties"]["action"];
        assert!(
            action.get("const").is_none(),
            "first branch's const must not survive as the only legal action: {action}"
        );
        assert_eq!(action["type"], "string");
        assert_eq!(
            action["enum"],
            serde_json::json!(["add", "remove", "batch"]),
            "every variant tag, in branch order"
        );
        // The sibling fields the tag used to contradict are all still here.
        let props = schema["properties"].as_object().expect("properties");
        assert!(props.contains_key("content"));
        assert!(props.contains_key("old_text"));
        assert!(props.contains_key("operations"));
    }

    #[test]
    fn envelope_widening_dedupes_and_reads_enum_branches() {
        let mut schema = serde_json::json!({
            "oneOf": [
                {"type": "object", "properties": {"kind": {"enum": ["a", "b"]}}},
                {"type": "object", "properties": {"kind": {"const": "b"}}},
                {"type": "object", "properties": {"kind": {"enum": ["c"]}}}
            ]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(
            schema["properties"]["kind"]["enum"],
            serde_json::json!(["a", "b", "c"]),
            "union of const + enum branches, branch order, deduped"
        );
    }

    #[test]
    fn envelope_leaves_key_alone_when_a_branch_does_not_pin_it() {
        // `mode` is pinned in the first branch but free-form in the second, so
        // it is not a discriminator — synthesizing an enum here would invent a
        // constraint the schema never had. First-branch-wins still applies.
        let mut schema = serde_json::json!({
            "oneOf": [
                {"type": "object", "properties": {"mode": {"type": "string", "const": "fast"}}},
                {"type": "object", "properties": {"mode": {"type": "string"}}}
            ]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert!(
            schema["properties"]["mode"].get("enum").is_none(),
            "no enum may be synthesized for a partially-pinned key"
        );
        assert_eq!(schema["properties"]["mode"]["const"], "fast");
    }

    #[test]
    fn envelope_keeps_lone_branch_const_untouched() {
        // Only one branch declares `only_here`, so its `const` is still exactly
        // true — widening a non-collision would be pure noise.
        let mut schema = serde_json::json!({
            "oneOf": [
                {"type": "object", "properties": {"only_here": {"type": "string", "const": "x"}}},
                {"type": "object", "properties": {"other": {"type": "integer"}}}
            ]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(schema["properties"]["only_here"]["const"], "x");
        assert!(schema["properties"]["only_here"].get("enum").is_none());
    }

    #[test]
    fn envelope_flattens_anyof_same_as_oneof() {
        let mut schema = serde_json::json!({
            "type": "object",
            "anyOf": [
                {"properties": {"a": {"type":"string"}}, "required":["a"]},
                {"properties": {"b": {"type":"integer"}}, "required":["b"]}
            ]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert!(schema.get("anyOf").is_none(), "anyOf removed");
        let props = schema["properties"].as_object().expect("properties");
        assert!(props.contains_key("a"));
        assert!(props.contains_key("b"));
        // Intersection of disjoint required sets is empty — `required` key may
        // be absent or empty array; both acceptable.
        match schema.get("required") {
            None => {}
            Some(Value::Array(arr)) => {
                assert!(arr.is_empty(), "disjoint intersection should be empty");
            }
            other => panic!("unexpected required: {other:?}"),
        }
    }

    #[test]
    fn envelope_strips_top_level_allof_enum_not() {
        let mut schema = serde_json::json!({
            "type": "object",
            "allOf": [{"type": "object"}],
            "enum": ["a", "b"],
            "not": {"type": "null"}
        });
        ensure_openai_tool_envelope(&mut schema);
        assert!(schema.get("allOf").is_none());
        assert!(schema.get("enum").is_none());
        assert!(schema.get("not").is_none());
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].is_object());
    }

    #[test]
    fn envelope_ensures_properties_exists() {
        let mut schema = serde_json::json!({"type": "object"});
        ensure_openai_tool_envelope(&mut schema);
        assert!(schema["properties"].is_object());
    }

    #[test]
    fn lenient_rewrite_null_pair_becomes_anyof() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"],
            "description": "optional label"
        });
        lenient_multi_type_rewrite(&mut schema);
        assert!(schema["anyOf"].is_array());
        assert_eq!(schema["anyOf"][1]["type"], "string");
        assert_eq!(schema["anyOf"][1]["description"], "optional label");
    }

    #[test]
    fn lenient_rewrite_any_value_strips_type() {
        let mut schema = serde_json::json!({
            "type": ["boolean", "object", "array", "number", "string", "integer", "null"],
            "description": "raw value"
        });
        lenient_multi_type_rewrite(&mut schema);
        assert!(
            schema.get("type").is_none(),
            "type stripped for non-null multi"
        );
        assert_eq!(schema["description"], "raw value", "sibling keys preserved");
    }

    #[test]
    fn lenient_rewrite_recurses_through_nesting() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "actions": {
                    "type": "array",
                    "items": {
                        "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
                    }
                }
            }
        });
        lenient_multi_type_rewrite(&mut schema);
        let items = &schema["properties"]["actions"]["items"];
        assert!(items.get("type").is_none(), "deep type stripped");
    }

    #[test]
    fn incompatible_short_circuits_through_nesting() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "actions": {
                    "type": "array",
                    "items": {
                        "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
                    }
                }
            }
        });
        let result = normalize_strict_schema(&mut schema, false);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(
                    reason.contains(".properties.actions.items"),
                    "reason should carry JSON-pointer path: {reason}"
                );
                assert!(reason.contains("multi-type"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
    }

    // =====================================================================
    // $ref / $defs strict incompatibility (per-tool non-strict downgrade)
    // =====================================================================

    #[test]
    fn remember_batch_shaped_defs_schema_returns_incompatible() {
        let mut schema = remember_args_shaped_schema();
        let result = normalize_strict_schema(&mut schema, true);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(reason.contains("$defs"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
        // The bail happens before any mutation — the schema must not have
        // been decorated with `strict: true` on the way out.
        assert!(
            schema.get("strict").is_none(),
            "rejected schema must never carry strict: true"
        );
    }

    #[test]
    fn remember_batch_shaped_schema_after_envelope_still_incompatible() {
        // The Responses path runs ensure_openai_tool_envelope BEFORE
        // normalize_strict_schema; the flatten removes the top-level oneOf
        // but keeps the $defs bucket and the $ref in operations.items.
        let mut schema = remember_args_shaped_schema();
        ensure_openai_tool_envelope(&mut schema);
        assert!(schema.get("$defs").is_some(), "envelope keeps $defs");
        let result = normalize_strict_schema(&mut schema, true);
        assert!(
            matches!(result, StrictResult::Incompatible { .. }),
            "post-envelope shape must still downgrade, got {result:?}"
        );
    }

    #[test]
    fn bare_ref_node_returns_incompatible_with_pointer_path() {
        // Even without a root $defs bucket (e.g. a caller stripped it), a
        // bare $ref node in a visited position must trigger the downgrade —
        // the target subschema is unreachable for normalization.
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/SingleOp" }
                }
            }
        });
        let result = normalize_strict_schema(&mut schema, false);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(
                    reason.contains(".properties.operations.items"),
                    "reason should carry JSON-pointer path: {reason}"
                );
                assert!(reason.contains("#/$defs/SingleOp"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
    }

    #[test]
    fn legacy_definitions_bucket_returns_incompatible() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "color": { "$ref": "#/definitions/Color" }
            },
            "definitions": {
                "Color": { "type": "string", "enum": ["red", "green"] }
            }
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert!(matches!(result, StrictResult::Incompatible { .. }));
    }

    #[test]
    fn deref_then_normalize_recovers_strict_for_defs_schema() {
        // Documents the escape hatch: inlining references first (as the
        // Moonshot policy path does) makes the schema strict-normalizable.
        let mut schema = remember_args_shaped_schema();
        deref_json_schema(&mut schema);
        ensure_openai_tool_envelope(&mut schema);
        let result = normalize_strict_schema(&mut schema, false);
        assert_eq!(result, StrictResult::Ok);
        let items = &schema["properties"]["operations"]["items"];
        assert!(items.get("$ref").is_none(), "ref inlined");
        assert_eq!(
            items["oneOf"][0]["additionalProperties"], false,
            "inlined subschemas get normalized"
        );
    }

    #[test]
    fn ref_free_schema_strict_result_byte_identical() {
        // Regression pin: the $ref/$defs guards must not perturb the output
        // for schemas without references. serde_json (without preserve_order)
        // serializes maps key-sorted, so string equality is byte-level.
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "label": { "type": ["null", "string"] },
                "user": {
                    "type": "object",
                    "properties": { "age": { "type": "integer" } }
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["user"]
        });
        let result = normalize_strict_schema(&mut schema, true);
        assert_eq!(result, StrictResult::Ok);
        let expected = serde_json::json!({
            "additionalProperties": false,
            "properties": {
                "label": {
                    "anyOf": [
                        { "type": "null" },
                        { "type": "string" }
                    ]
                },
                "user": {
                    "additionalProperties": false,
                    "properties": { "age": { "type": "integer" } },
                    "type": "object"
                },
                "tags": {
                    "items": { "type": "string" },
                    "type": "array"
                }
            },
            "required": ["user"],
            "strict": true,
            "type": "object"
        });
        assert_eq!(schema, expected);
        assert_eq!(
            serde_json::to_string(&schema).unwrap(),
            serde_json::to_string(&expected).unwrap(),
            "byte-identical output for $ref-free schemas"
        );
    }

    // =====================================================================
    // Moonshot (Kimi) compatibility tests
    // =====================================================================

    #[test]
    fn deref_inlines_local_refs_and_removes_defs() {
        let mut schema = serde_json::json!({
            "$defs": {
                "Color": {
                    "type": "string",
                    "enum": ["red", "green", "blue"]
                }
            },
            "type": "object",
            "properties": {
                "favorite": { "$ref": "#/$defs/Color" }
            }
        });
        deref_json_schema(&mut schema);
        assert!(schema.get("$defs").is_none());
        assert!(schema["properties"]["favorite"].get("$ref").is_none());
        assert_eq!(schema["properties"]["favorite"]["type"], "string");
    }

    #[test]
    fn deref_preserves_remote_refs() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "remote": { "$ref": "https://example.com/schema" }
            }
        });
        deref_json_schema(&mut schema);
        assert_eq!(
            schema["properties"]["remote"]["$ref"],
            "https://example.com/schema"
        );
    }

    #[test]
    fn deref_handles_nested_refs() {
        let mut schema = serde_json::json!({
            "$defs": {
                "Inner": { "type": "integer" },
                "Outer": {
                    "type": "object",
                    "properties": {
                        "value": { "$ref": "#/$defs/Inner" }
                    }
                }
            },
            "type": "object",
            "properties": {
                "outer": { "$ref": "#/$defs/Outer" }
            }
        });
        deref_json_schema(&mut schema);
        assert_eq!(
            schema["properties"]["outer"]["properties"]["value"]["type"],
            "integer"
        );
    }

    #[test]
    fn ensure_property_types_adds_type_to_enum_property() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "size": { "enum": ["small", "medium", "large"] }
            }
        });
        ensure_property_types(&mut schema);
        assert_eq!(schema["properties"]["size"]["type"], "string");
    }

    #[test]
    fn ensure_property_types_adds_type_from_const() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "const": "widget" }
            }
        });
        ensure_property_types(&mut schema);
        assert_eq!(schema["properties"]["kind"]["type"], "string");
    }

    #[test]
    fn ensure_property_types_infers_object_from_properties() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "nested": {
                    "properties": { "a": { "type": "string" } },
                    "required": ["a"]
                }
            }
        });
        ensure_property_types(&mut schema);
        assert_eq!(schema["properties"]["nested"]["type"], "object");
    }

    #[test]
    fn ensure_property_types_skips_combinator_nodes() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "choice": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "integer" }
                    ]
                }
            }
        });
        ensure_property_types(&mut schema);
        assert!(schema["properties"]["choice"].get("type").is_none());
    }

    #[test]
    fn moonshot_normalization_of_action_ref() {
        // Mimics the `loop` tool schema: an `action` property that schemars
        // emits as a `$ref` to a `$defs` enum. Moonshot rejects this unless
        // we inline the reference and ensure explicit types.
        let mut schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": {
                "LoopAction": {
                    "oneOf": [
                        { "type": "string", "const": "start" },
                        { "type": "string", "const": "stop" }
                    ]
                }
            },
            "type": "object",
            "properties": {
                "action": { "$ref": "#/$defs/LoopAction" }
            },
            "required": ["action"]
        });
        deref_json_schema(&mut schema);
        ensure_property_types(&mut schema);
        assert!(schema.get("$defs").is_none());
        assert!(schema["properties"]["action"].get("$ref").is_none());
        // The inlined `oneOf` branches already have explicit types; the outer
        // `action` schema itself does not need a type because it uses `oneOf`.
        assert!(schema["properties"]["action"]["oneOf"].is_array());
    }
}
