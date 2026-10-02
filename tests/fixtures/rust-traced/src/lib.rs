use std::fmt::Display;

pub mod billing;
pub mod inventory;
pub mod model;

pub trait Audit {}

pub trait Export {}

pub type Amount = u64;

pub fn audited_len<T: Audit>(items: &[T]) -> usize {
    items.len()
}

pub fn audited_count<U>(values: &[U]) -> usize
where
    U: Audit,
{
    values.len()
}

pub fn exported_len<T: Export>(items: &[T]) -> usize {
    items.len()
}

pub fn displayed_len<T: Display>(items: &[T]) -> usize {
    items.len()
}

pub fn shown_count<U: Display>(values: &[U]) -> usize {
    values.len()
}

pub fn charged(amount: Amount) -> Amount {
    amount + 1
}

pub fn billed(total: Amount) -> Amount {
    total + 1
}

pub fn raw(amount: u64) -> u64 {
    amount + 1
}

pub struct Ledger;

impl Ledger {
    pub fn total(&self, amount: Amount) -> Amount {
        amount + 1
    }
}
