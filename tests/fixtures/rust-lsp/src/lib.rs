pub fn first(value: i32) -> i32 {
    (value + 1) * (value - 1) / 2
}

pub fn second(value: i32) -> i32 {
    (value + 2) * (value - 2) / 2
}

pub fn use_both() -> i32 {
    first(1) + second(2)
}
