// 验证 device_from_identifier 对各种输入的返回值。
// 重点是空串和全斜杠——原实现的 `unwrap_or("device")` 兜底永远不会触发，
// 因为 `split('/')` 对空输入也会产出一个空元素。

fn device_from_identifier(id: &str) -> String {
    let mut parts = id
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty());

    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => format!("{a}/{b}"),
        (Some(a), None) => a.to_string(),
        _ => "device".to_string(),
    }
}

#[test]
fn empty_string_falls_back_to_device() {
    assert_eq!(device_from_identifier(""), "device");
}

#[test]
fn only_slashes_falls_back_to_device() {
    assert_eq!(device_from_identifier("/"), "device");
    assert_eq!(device_from_identifier("///"), "device");
}

#[test]
fn single_segment_returns_itself() {
    assert_eq!(device_from_identifier("amdcpu"), "amdcpu");
    assert_eq!(device_from_identifier("/ram"), "ram");
}

#[test]
fn two_segments_joined() {
    assert_eq!(device_from_identifier("amdcpu/0"), "amdcpu/0");
    assert_eq!(device_from_identifier("/gpu-nvidia/0"), "gpu-nvidia/0");
}

#[test]
fn full_sensor_id_takes_first_two() {
    assert_eq!(
        device_from_identifier("/amdcpu/0/temperature/2"),
        "amdcpu/0"
    );
    assert_eq!(
        device_from_identifier("/nvme/1/temperature/0"),
        "nvme/1"
    );
    assert_eq!(device_from_identifier("/ram/load/0"), "ram/load");
}

#[test]
fn leading_slashes_ignored() {
    assert_eq!(device_from_identifier("//amdcpu/0/load/0"), "amdcpu/0");
}
