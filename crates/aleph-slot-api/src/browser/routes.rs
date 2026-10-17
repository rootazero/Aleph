//! Public browser route DTOs.

/// A rule's decision material. `Mock` carries everything `Fetch.fulfillRequest`
/// needs, so the loop never re-consults the registry between verdict and wire.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RouteKind {
    Mock {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Abort,
}

/// Who a rule applies to. `Tab` binds the one tab it was added against;
/// `Profile` serves every tab of ITS profile — never another profile's
/// (spec §4: "profile 下所有 tab", and a mock that bled across principals
/// would be an isolation breach, not a feature).
///
/// Serialized in `browser_network`'s tool output and carried by the
/// canonical browser-port route values in both directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteScope {
    Tab,
    Profile,
}

/// Everything `route_add` needs beyond WHERE the rule binds (the backend
/// supplies the profile from its own identity; the call supplies the tab).
/// Grouped because the trait signature would otherwise be seven positional
/// parameters wide.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NewRouteRule {
    pub url_contains: String,
    pub method: Option<String>,
    pub kind: RouteKind,
    pub scope: RouteScope,
    pub note: Option<String>,
}

/// What `mock_list` shows. A snapshot struct rather than a borrow so the
/// registry lock is never held across a read the caller formats.
///
/// This struct is `browser_network`'s wire answer and a browser-port reply —
/// `kind` travels as the label, never the material (the body can be 256 KiB
/// and the model that listed the rules already knows what it wrote).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct RouteRuleInfo {
    pub id: String,
    pub url_contains: String,
    pub method: Option<String>,
    pub kind_label: &'static str,
    pub scope: RouteScope,
    pub hits: u64,
    /// Whether a live interception loop is currently serving this rule. A rule
    /// whose loop died (session death, engine restart) stays in the table but
    /// reads `active: false` — present-but-unserved is a fact the model must
    /// be able to see, not a success we invent (判据 §8).
    pub active: bool,
    /// The model's own annotation, carried back verbatim by `mock_list`.
    pub note: Option<String>,
}

impl<'de> serde::Deserialize<'de> for RouteRuleInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        enum Field {
            Id,
            UrlContains,
            Method,
            KindLabel,
            Scope,
            Hits,
            Active,
            Note,
            Ignore,
        }

        impl<'de> serde::Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct FieldVisitor;

                impl<'de> serde::de::Visitor<'de> for FieldVisitor {
                    type Value = Field;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("a RouteRuleInfo field name")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "id" => Ok(Field::Id),
                            "url_contains" => Ok(Field::UrlContains),
                            "method" => Ok(Field::Method),
                            "kind_label" => Ok(Field::KindLabel),
                            "scope" => Ok(Field::Scope),
                            "hits" => Ok(Field::Hits),
                            "active" => Ok(Field::Active),
                            "note" => Ok(Field::Note),
                            _ => Ok(Field::Ignore),
                        }
                    }

                    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            b"id" => Ok(Field::Id),
                            b"url_contains" => Ok(Field::UrlContains),
                            b"method" => Ok(Field::Method),
                            b"kind_label" => Ok(Field::KindLabel),
                            b"scope" => Ok(Field::Scope),
                            b"hits" => Ok(Field::Hits),
                            b"active" => Ok(Field::Active),
                            b"note" => Ok(Field::Note),
                            _ => Ok(Field::Ignore),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct RouteRuleInfoVisitor;

        impl<'de> serde::de::Visitor<'de> for RouteRuleInfoVisitor {
            type Value = RouteRuleInfo;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct RouteRuleInfo")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let id: String = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let url_contains: String = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let method: Option<String> = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(2, &self))?;
                let kind_label: String = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(3, &self))?;
                let kind_label: &'static str = match kind_label.as_str() {
                    "mock" => "mock",
                    "abort" => "abort",
                    _ => {
                        return Err(serde::de::Error::unknown_variant(
                            &kind_label,
                            &["mock", "abort"],
                        ));
                    }
                };
                let scope: RouteScope = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(4, &self))?;
                let hits: u64 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(5, &self))?;
                let active: bool = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(6, &self))?;
                let note: Option<String> = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(7, &self))?;
                Ok(RouteRuleInfo {
                    id,
                    url_contains,
                    method,
                    kind_label,
                    scope,
                    hits,
                    active,
                    note,
                })
            }

            fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut id: Option<String> = None;
                let mut url_contains: Option<String> = None;
                let mut method: Option<Option<String>> = None;
                let mut kind_label: Option<&'static str> = None;
                let mut scope: Option<RouteScope> = None;
                let mut hits: Option<u64> = None;
                let mut active: Option<bool> = None;
                let mut note: Option<Option<String>> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => {
                            if id.is_some() {
                                return Err(serde::de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::UrlContains => {
                            if url_contains.is_some() {
                                return Err(serde::de::Error::duplicate_field("url_contains"));
                            }
                            url_contains = Some(map.next_value()?);
                        }
                        Field::Method => {
                            if method.is_some() {
                                return Err(serde::de::Error::duplicate_field("method"));
                            }
                            method = Some(map.next_value()?);
                        }
                        Field::KindLabel => {
                            if kind_label.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind_label"));
                            }
                            let label: String = map.next_value()?;
                            kind_label = Some(match label.as_str() {
                                "mock" => "mock",
                                "abort" => "abort",
                                _ => {
                                    return Err(serde::de::Error::unknown_variant(
                                        &label,
                                        &["mock", "abort"],
                                    ));
                                }
                            });
                        }
                        Field::Scope => {
                            if scope.is_some() {
                                return Err(serde::de::Error::duplicate_field("scope"));
                            }
                            scope = Some(map.next_value()?);
                        }
                        Field::Hits => {
                            if hits.is_some() {
                                return Err(serde::de::Error::duplicate_field("hits"));
                            }
                            hits = Some(map.next_value()?);
                        }
                        Field::Active => {
                            if active.is_some() {
                                return Err(serde::de::Error::duplicate_field("active"));
                            }
                            active = Some(map.next_value()?);
                        }
                        Field::Note => {
                            if note.is_some() {
                                return Err(serde::de::Error::duplicate_field("note"));
                            }
                            note = Some(map.next_value()?);
                        }
                        Field::Ignore => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                Ok(RouteRuleInfo {
                    id: id.ok_or_else(|| serde::de::Error::missing_field("id"))?,
                    url_contains: url_contains
                        .ok_or_else(|| serde::de::Error::missing_field("url_contains"))?,
                    method: method.unwrap_or_default(),
                    kind_label: kind_label
                        .ok_or_else(|| serde::de::Error::missing_field("kind_label"))?,
                    scope: scope.ok_or_else(|| serde::de::Error::missing_field("scope"))?,
                    hits: hits.ok_or_else(|| serde::de::Error::missing_field("hits"))?,
                    active: active.ok_or_else(|| serde::de::Error::missing_field("active"))?,
                    note: note.unwrap_or_default(),
                })
            }
        }

        const FIELDS: &[&str] = &[
            "id",
            "url_contains",
            "method",
            "kind_label",
            "scope",
            "hits",
            "active",
            "note",
        ];
        deserializer.deserialize_struct("RouteRuleInfo", FIELDS, RouteRuleInfoVisitor)
    }
}
