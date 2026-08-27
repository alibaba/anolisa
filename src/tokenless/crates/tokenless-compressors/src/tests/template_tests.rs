#[test]
fn prefix_through_first_colon_stays_verbatim() {
    assert_eq!(mask("app: user 42 connected from 10.0.0.7"), "app: user 0 connected from 0.0.0.0");
    // Only the first delimiter splits; later colons are part of the suffix.
    assert_eq!(mask("path/to/file.rs:123: note"), "path/to/file.rs:0: note");
}

#[test]
fn equals_is_a_delimiter_too() {
    assert_eq!(mask("count=12345"), "count=0");
}

#[test]
fn earlier_delimiter_wins() {
    assert_eq!(mask("a=b: c 9"), "a=b: c 0");
}

#[test]
fn long_hex_runs_collapse_after_the_delimiter() {
    assert_eq!(mask("id: deadbeef1234 done"), "id: h done");
    // Short hex-letter runs are ordinary prose and stay verbatim.
    assert_eq!(mask("mode: fade 3"), "mode: fade 0");
}

#[test]
fn line_without_delimiter_is_identity() {
    let line = "Compiling foo v1.2.3";
    assert_eq!(mask(line), line);
}

#[test]
fn digits_before_the_delimiter_stay_verbatim() {
    assert_eq!(mask("worker 7 says: took 12ms"), "worker 7 says: took 0ms");
}

#[test]
fn top_templates_groups_and_orders_deterministically() {
    let lines: Vec<String> = (0..6)
        .map(|i| format!("npm http fetch GET 200 https://r.npmjs.org/pkg {i}ms"))
        .chain((0..5).map(|i| format!("cache: hit {i}")))
        .chain(std::iter::once("unique line".to_string()))
        .collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let groups = top_templates(&refs, 5, 3);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].0, 6);
    assert_eq!(groups[0].1, "npm http fetch GET 200 https://r.npmjs.org/pkg 0ms");
    assert_eq!(groups[1], (5, "cache: hit 0".to_string()));
}

#[test]
fn top_templates_respects_min_count_and_top_k() {
    let lines: Vec<String> = (0..4).map(|i| format!("below: {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(top_templates(&refs, 5, 3).is_empty());

    let many: Vec<String> = (0..4)
        .flat_map(|group| (0..5 + group).map(move |i| format!("g{group}: item {i}")))
        .collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let groups = top_templates(&refs, 5, 3);
    assert_eq!(groups.len(), 3);
    assert!(groups[0].0 >= groups[1].0 && groups[1].0 >= groups[2].0);
}
