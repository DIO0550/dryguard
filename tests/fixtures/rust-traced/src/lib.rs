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

pub fn imported_alias(amount: model::Cost) -> model::Cost {
    amount + 1
}

pub fn small_alias(amount: model::Small) -> model::Small {
    amount + 1
}

pub fn generic_alias(value: model::Pair<u8>) -> u8 {
    value.0
}

pub fn named_alias(value: model::CustomerAlias) -> model::CustomerAlias {
    value
}

pub fn counted_alias(value: model::Counted) -> model::Counted {
    value
}

pub mod shadowed {
    pub struct Number;
    use Number as u64;
    pub type Amount = u64;
    pub type Pair = (u64, u8);

    pub fn shadowed_pair(value: Pair) -> Pair {
        value
    }

    pub fn shadowed_alias(value: Amount) -> Amount {
        value
    }
}

pub type Packet = (u8, u64);
pub type Callback = fn(u8) -> u64;

pub fn compound_alias(value: &Packet, callback: Callback) -> u64 {
    callback(value.0) + value.1
}

pub fn compound_raw(value: &(u8, u64), callback: fn(u8) -> u64) -> u64 {
    callback(value.0) + value.1
}

pub mod shadowed_by_alias {
    pub type Byte = u8;
    use Byte as u64;
    pub type Amount = u64;

    pub fn shadowed_by_alias(value: Amount) -> Amount {
        value
    }
}

pub mod renamed_primitive {
    use std::primitive::u8 as u64;
    pub type Amount = u64;

    pub fn renamed_primitive(value: Amount) -> Amount {
        value
    }
}

pub mod impls;
