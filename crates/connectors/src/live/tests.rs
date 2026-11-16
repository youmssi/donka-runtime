use super::*;

#[test]
fn backoff_doubles_up_to_a_second() {
    assert_eq!(backoff(1), Duration::from_millis(100));
    assert_eq!(backoff(2), Duration::from_millis(200));
    assert_eq!(backoff(3), Duration::from_millis(400));
    assert_eq!(backoff(9), Duration::from_millis(1000));
}
