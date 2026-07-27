use super::skills::render_skills_command;
use crate::runtime::prelude::*;

fn zh_state() -> InlineState {
    InlineState {
        language: Language::ZhCn,
        ..InlineState::default()
    }
}

fn en_state() -> InlineState {
    InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    }
}

#[test]
fn skills_non_cosh_core_shows_unavailable_zh() {
    let adapter = AdapterInstance::Fake(crate::adapter::FakeAgentAdapter);
    let mut state = zh_state();
    let mut buf = Vec::new();
    render_skills_command(None, None, &adapter, &mut state, &mut buf).unwrap();
    let output = String::from_utf8(buf).unwrap();
    assert!(
        output.contains("cosh-core") || output.contains("后端"),
        "should contain degradation message: {output}"
    );
}

#[test]
fn skills_non_cosh_core_shows_unavailable_en() {
    let adapter = AdapterInstance::Fake(crate::adapter::FakeAgentAdapter);
    let mut state = en_state();
    let mut buf = Vec::new();
    render_skills_command(None, None, &adapter, &mut state, &mut buf).unwrap();
    let output = String::from_utf8(buf).unwrap();
    assert!(
        output.contains("cosh-core backend"),
        "should contain English degradation message: {output}"
    );
}
