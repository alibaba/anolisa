#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputExcerptDirection {
    Head,
    Tail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvidenceExcerptRequest {
    pub(crate) output_id: String,
    pub(crate) direction: OutputExcerptDirection,
    pub(crate) lines: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvidenceExcerpt {
    pub(crate) text: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) redaction_status: &'static str,
    pub(crate) truncated: bool,
}

pub(crate) type OutputExcerptRequest = EvidenceExcerptRequest;
