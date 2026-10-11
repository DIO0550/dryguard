pub mod far;

pub struct Holder<T>(pub T);
pub struct Plain;
pub struct Unique;

pub trait Other<A> {
    type Item;
    type Wrap<U>;
}
impl<X> Other<u8> for Holder<X> {
    type Item = (X, u8);
    type Wrap<U> = (U, X);
}
impl<X> Other<u16> for Holder<X> {
    type Item = u16;
    type Wrap<U> = U;
}
pub trait Echo {
    type Me;
}
impl<X> Echo for Holder<X> {
    type Me = (Self, X);
}
pub mod same_name {
    pub trait Other<A> {
        type Item;
    }
    impl<X> Other<u8> for super::Holder<X> {
        type Item = u64;
    }
}
pub trait Blank {
    type Out;
}
impl<T: Clone> Blank for T {
    type Out = T;
}
impl Clone for Plain {
    fn clone(&self) -> Self {
        Plain
    }
}
impl ToOwned for Unique {
    type Owned = Box<Unique>;
    fn to_owned(&self) -> Box<Unique> {
        Box::new(Unique)
    }
}
pub trait Anything {
    type Same;
}
impl<T> Anything for T {
    type Same = (T, u8);
}
pub struct Bytes([u8]);
pub trait Every {
    type Out;
}
impl<T> Every for T {
    type Out = u8;
}
// 暗黙の `T: Sized` により上の blanket impl と共存する。属性付きなので候補として読まない
#[doc(hidden)]
impl Every for Bytes {
    type Out = u16;
}
pub struct Slice([u8]);
// 暗黙の `T: Sized` により Every の blanket impl と共存する。属性が無いので候補として読む
impl Every for Slice {
    type Out = u16;
}
pub trait Show {
    fn shown(value: u16) -> u16;
    fn shown_plain(value: u16) -> u16;
}

impl<T> Holder<T> {
    pub fn inherent(value: <Self as Other<u8>>::Item) -> <Self as Other<u8>>::Item { value }
    pub fn inherent_plain(value: (T, u8)) -> (T, u8) { value }
    pub fn sixteen(value: <Self as Other<u16>>::Item) -> u16 { value }
    pub fn sixteen_plain(value: u16) -> u16 { value }
    pub fn wrapped<V>(value: <Self as Other<u8>>::Wrap<V>) -> (V, T) { value }
    pub fn wrapped_plain<V>(value: (V, T)) -> (V, T) { value }
    pub fn renamed(value: <Self as same_name::Other<u8>>::Item) -> u64 { value }
    pub fn renamed_plain(value: u64) -> u64 { value }
    pub fn distant(value: <Self as far::Far>::Out) -> Vec<T> { value }
    pub fn distant_plain(value: Vec<T>) -> Vec<T> { value }
    pub fn echoed(value: <Self as Echo>::Me) -> T { value.1 }
    pub fn echoed_plain(value: (Holder<T>, T)) -> T { value.1 }
    pub fn bounded(value: <Self as Other<u8>>::Item) -> <Self as Other<u8>>::Item where Self: Other<u8> { value }
}
impl<T> Show for Holder<T> {
    fn shown(value: <Self as Other<u16>>::Item) -> u16 { value }
    fn shown_plain(value: u16) -> u16 { value }
}
impl Holder<u8> {
    pub fn concrete(value: <Self as Other<u16>>::Item) -> u16 { value }
    pub fn concrete_plain(value: u16) -> u16 { value }
    pub fn eight(value: <Self as Other<u8>>::Item) -> (u8, u8) { value }
    pub fn eight_plain(value: (u8, u8)) -> (u8, u8) { value }
}
impl<T> Holder<Vec<T>> {
    pub fn nested(value: <Self as Other<u8>>::Item) -> (Vec<T>, u8) { value }
    pub fn nested_plain(value: (Vec<T>, u8)) -> (Vec<T>, u8) { value }
}
impl Holder<Plain> {
    pub fn declared(value: <Self as Other<u8>>::Item) {}
    pub fn declared_plain(value: (Plain, u8)) {}
}
impl Plain {
    pub fn blanket(value: <Self as Blank>::Out) -> Plain { value }
    pub fn anything(value: <Self as Anything>::Same) -> (Plain, u8) { value }
    pub fn anything_plain(value: (Plain, u8)) -> (Plain, u8) { value }
}
impl Unique {
    pub fn owned(value: <Self as ToOwned>::Owned) -> Box<Unique> { value }
    pub fn owned_plain(value: Box<Unique>) -> Box<Unique> { value }
}
impl Bytes {
    pub fn every(value: <Self as Every>::Out) -> u16 { value }
}
impl Slice {
    pub fn slice(value: <Self as Every>::Out) -> u16 { value }
    pub fn slice_plain(value: u16) -> u16 { value }
}
