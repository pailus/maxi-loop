/// Scope & matching rule for configurable PII scrubbing (additive to mandatory baseline scrubbing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiiScrubRuleType {
    FieldName,
    HeaderName,
    ValueSubstring,
}

impl PiiScrubRuleType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FieldName => "field_name",
            Self::HeaderName => "header_name",
            Self::ValueSubstring => "value_substring",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "field_name" => Some(Self::FieldName),
            "header_name" => Some(Self::HeaderName),
            "value_substring" => Some(Self::ValueSubstring),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiiScrubRuleEntry {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: Option<i64>,
    pub project_id: Option<i64>,
    pub rule_type: PiiScrubRuleType,
    pub pattern: String,
    pub replacement: String,
    pub is_enabled: bool,
}

/// Observer trait allowing the in-memory ingest PII scrub rules cache to stay synchronized
/// when rules are created, updated, or deleted without coupling `org` to `ingest`.
pub trait PiiScrubRulesObserver: Send + Sync {
    fn on_pii_rule_upserted(&self, rule: PiiScrubRuleEntry);
    fn on_pii_rule_deleted(&self, tenant_id: i64, rule_id: i64);
}
