use crate::shared::{math::scale as convert, tax};

pub fn discount(amount: i32) -> i32 {
    (amount + 1) * (amount - 1) / 2
}

struct Invoice;
impl Invoice {
    fn total(amount: i32) -> i32 {
        (amount + 1) * (amount - 1) / 2
    }
}

trait Rule {
    fn apply(amount: i32) -> i32;
}
impl Rule for Invoice {
    fn apply(amount: i32) -> i32 {
        (amount + 1) * (amount - 1) / 2
    }
}
