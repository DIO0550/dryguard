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
}
impl Plain {
    pub fn blanket(value: <Self as Blank>::Out) -> Plain { value }
}
impl Unique {
    pub fn owned(value: <Self as ToOwned>::Owned) -> Box<Unique> { value }
    pub fn owned_plain(value: Box<Unique>) -> Box<Unique> { value }
}
