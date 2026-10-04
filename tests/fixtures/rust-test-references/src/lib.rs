mod report;

pub fn first(value: i32) -> i32 {
    (value + 1) * (value - 1) / 2
}

pub fn second(value: i32) -> i32 {
    (value + 2) * (value - 2) / 2
}

pub fn production() -> i32 {
    first(1) + first(2) + second(3)
}

#[test]
fn check() {
    assert_eq!(first(1) + second(2), 0);
}
