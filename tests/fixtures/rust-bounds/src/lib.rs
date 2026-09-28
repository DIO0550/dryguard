use std::fmt::Display;

pub fn first_len<T>(items: &[T]) -> usize {
    items.len()
}

pub fn second_len<U>(values: &[U]) -> usize {
    values.len()
}

pub fn borrowed_size<'a, T: 'a + ?Sized>(item: &'a T) -> usize {
    std::mem::size_of_val(item)
}

pub fn elided_size<U>(value: &U) -> usize
where
    U: ?Sized,
{
    std::mem::size_of_val(value)
}

pub fn displayed_len<T: Display>(items: &[T]) -> usize {
    items.len()
}
