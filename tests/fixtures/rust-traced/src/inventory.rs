use crate::model::Customer;

pub struct User;

pub fn named(user: User) -> usize {
    let _ = user;
    1
}

pub fn greeted(customer: Customer) -> usize {
    let _ = customer;
    1
}
