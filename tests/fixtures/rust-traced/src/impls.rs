pub trait Bound {}
pub trait Other {}
pub struct Holder<T>(T);
pub struct Different<T>(T);

impl<T: Bound> Holder<T> {
    pub fn wrap_a(value: T) -> Self { Self(value) }
    pub fn borrow_a(&self) -> &T { &self.0 }
    pub fn mutable(&mut self) -> &T { &self.0 }
    pub fn owned(self) -> T { self.0 }
    pub fn owned_mut(mut self) -> T { self.0 }
    pub fn boxed(self: Box<Self>) -> T { self.0 }
    pub fn mixed_a<U>(x: T) -> U { todo!() }
    pub fn mixed_b<U>(x: U) -> T { todo!() }
    pub fn shadow<Bound>(&self, x: Bound) -> Bound { x }
    pub fn target_shadow<Holder>(&self, x: Holder) -> Holder { x }
}
impl<U> Holder<U> where U: Bound {
    pub fn wrap_b(value: U) -> Holder<U> { Holder(value) }
    pub fn borrow_b(&self) -> &U { &self.0 }
    pub fn shadow_renamed<V>(&self, x: V) -> V { x }
    pub fn explicit_box(value: Box<Holder<U>>) -> U { value.0 }
}
impl<T: Other> Holder<T> {
    pub fn other_bound(&self) -> &T { &self.0 }
}
impl<T: Bound> Different<T> {
    pub fn different_target(&self) -> &T { &self.0 }
}

pub trait Project {
    type Item;
    fn projected(&self) -> Self::Item;
}
impl<T: Bound> Project for Holder<T> {
    type Item = u8;
    fn projected(&self) -> Self::Item { 0 }
}

pub mod first {
    pub struct Twin;
    impl Twin {
        pub fn first_twin(&self) {}
    }
}
pub mod second {
    pub struct Twin;
    impl Twin {
        pub fn second_twin(&self) {}
    }
}

pub trait Access {
    fn access(&self) -> u8;
}
impl<T: Bound> Access for Holder<T> {
    fn access(&self) -> u8 { 0 }
}
