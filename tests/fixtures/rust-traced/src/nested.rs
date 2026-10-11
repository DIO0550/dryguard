pub mod far;

pub struct Holder<T>(pub T);
pub struct Leaf;
pub struct Branch;

pub trait Deep {
    type Out;
    type Me;
    type Wrap<U>;
}
impl Deep for Leaf {
    type Out = u16;
    type Me = (Self, u8);
    type Wrap<U> = (U, Self);
}
impl<T> Deep for Holder<T> {
    type Out = T;
    type Me = Self;
    type Wrap<U> = U;
}
impl Deep for Branch {
    type Out = i16;
    type Me = Self;
    type Wrap<U> = U;
}
pub trait Show {
    type Item;
    type Again;
    type Next;
    type Pair;
    type Twice;
    type Echo;
    type Through;
    type Generic;
    type Away;
    fn again(value: Self::Again) -> Self::Again;
    fn again_plain(value: Leaf) -> Leaf;
}
impl<T> Show for Holder<T> {
    type Item = Leaf;
    type Again = Self::Item;
    type Next = <Self::Item as Deep>::Out;
    type Pair = (Self::Item, T);
    type Twice = (Self::Echo, u8);
    type Echo = Self;
    type Through = (<Self as Show>::Item, Self);
    type Generic = Holder<u8>;
    type Away = far::Remote;
    fn again(value: Self::Again) -> Self::Again { value }
    fn again_plain(value: Leaf) -> Leaf { value }
}
pub trait Open {
    type Item;
}
impl<T> Open for Holder<T> {
    type Item = T;
}
pub trait Cycle {
    type First;
    type Second;
}
impl<T> Cycle for Holder<T> {
    type First = Self::Second;
    type Second = Self::First;
}
pub trait Chain {
    type C0;
    type C1;
    type C2;
    type C3;
    type C4;
    type C5;
    type C6;
    type C7;
    type C8;
    type C9;
    type C10;
    type C11;
    type C12;
    type C13;
    type C14;
    type C15;
    type C16;
    type C17;
    type C18;
    type C19;
    type C20;
    type C21;
    type C22;
    type C23;
    type C24;
    type C25;
    type C26;
    type C27;
    type C28;
    type C29;
    type C30;
    type C31;
    type C32;
}
impl<T> Chain for Holder<T> {
    type C0 = Self::C1;
    type C1 = Self::C2;
    type C2 = Self::C3;
    type C3 = Self::C4;
    type C4 = Self::C5;
    type C5 = Self::C6;
    type C6 = Self::C7;
    type C7 = Self::C8;
    type C8 = Self::C9;
    type C9 = Self::C10;
    type C10 = Self::C11;
    type C11 = Self::C12;
    type C12 = Self::C13;
    type C13 = Self::C14;
    type C14 = Self::C15;
    type C15 = Self::C16;
    type C16 = Self::C17;
    type C17 = Self::C18;
    type C18 = Self::C19;
    type C19 = Self::C20;
    type C20 = Self::C21;
    type C21 = Self::C22;
    type C22 = Self::C23;
    type C23 = Self::C24;
    type C24 = Self::C25;
    type C25 = Self::C26;
    type C26 = Self::C27;
    type C27 = Self::C28;
    type C28 = Self::C29;
    type C29 = Self::C30;
    type C30 = Self::C31;
    type C31 = Self::C32;
    type C32 = u8;
}
impl<T> Holder<T> {
    pub fn nested(value: <<Self as Show>::Item as Deep>::Out) -> u16 { value }
    pub fn nested_plain(value: u16) -> u16 { value }
    pub fn branch_plain(value: i16) -> u16 { value as u16 }
    pub fn me(value: <<Self as Show>::Item as Deep>::Me) {}
    pub fn me_plain(value: (Leaf, u8)) {}
    pub fn wrap<U>(value: <<Self as Show>::Item as Deep>::Wrap<U>) {}
    pub fn wrap_plain<U>(value: (U, Leaf)) {}
    pub fn next(value: <Self as Show>::Next) -> u16 { value }
    pub fn pair(value: <Self as Show>::Pair) {}
    pub fn pair_plain(value: (Leaf, T)) {}
    pub fn twice(value: <Self as Show>::Twice) {}
    pub fn twice_plain(value: (Holder<T>, u8)) {}
    pub fn through(value: <Self as Show>::Through) {}
    pub fn through_plain(value: (Leaf, Holder<T>)) {}
    pub fn generic(value: <<Self as Show>::Generic as Deep>::Out) {}
    pub fn open(value: <<Self as Open>::Item as Deep>::Out) {}
    pub fn cycle(value: <Self as Cycle>::First) {}
    pub fn chain(value: <Self as Chain>::C0) {}
    pub fn away(value: <<Self as Show>::Away as Deep>::Out) -> i32 { value }
    pub fn away_plain(value: i32) -> i32 { value }
    pub fn chain_fits(value: <Self as Chain>::C1) -> u8 { value }
    pub fn chain_plain(value: u8) -> u8 { value }
}
