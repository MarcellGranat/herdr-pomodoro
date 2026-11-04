use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct LiveMeasurements {
    human_input: HumanInput,
    configured_correlation_window_ns: i64,
    idle_access_time_changed: bool,
    output_modification_time_changed: bool,
    programmatic_pane_input_observations: i64,
    api_workspace_navigation_observations: i64,
    human_navigation_only_observations: i64,
    human_paste_observations: i64,
    human_active_segments_after_paste: i64,
    human_active_ns_after_paste: i64,
}

#[derive(Debug, Deserialize)]
struct HumanInput {
    absolute_delta_ns: i64,
}

#[test]
fn recorded_live_measurements_support_fail_closed_contract() {
    let values: LiveMeasurements =
        serde_json::from_str(include_str!("../tests/fixtures/live_pty_measurements.json")).unwrap();
    assert!(values.human_input.absolute_delta_ns <= values.configured_correlation_window_ns);
    assert!(!values.idle_access_time_changed);
    assert!(values.output_modification_time_changed);
    assert_eq!(values.programmatic_pane_input_observations, 0);
    assert_eq!(values.api_workspace_navigation_observations, 0);
    assert_eq!(values.human_navigation_only_observations, 0);
    assert!(values.human_paste_observations > 1);
    assert!(values.human_active_segments_after_paste > 0);
    assert!(values.human_active_ns_after_paste > 0);
}
