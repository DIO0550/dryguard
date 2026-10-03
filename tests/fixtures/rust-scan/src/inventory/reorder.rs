use crate::shared::math::scale;

pub fn reorder(quantity: i32) -> i32 {
    (quantity + 1) * (quantity - 1) / 2
}

struct Stock;
impl Stock {
    fn count(quantity: i32) -> i32 {
        (quantity + 1) * (quantity - 1) / 2
    }
}

trait Action {
    fn run(quantity: i32) -> i32;
}
impl Action for Stock {
    fn run(quantity: i32) -> i32 {
        (quantity + 1) * (quantity - 1) / 2
    }
}
