#![cfg(test)]

fn helper() -> i32 {
    crate::first(1) + crate::second(2)
}

#[test]
fn check() {
    assert_eq!(helper(), 0);
}
