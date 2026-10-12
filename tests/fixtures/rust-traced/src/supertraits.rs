pub mod far;

pub struct Holder<T>(pub T);
#[derive(Clone)]
pub struct Leaf;
pub struct Counter;
pub struct Bounded<T>(pub T);

pub trait Show {
    type Item;
}
impl Show for Leaf {
    type Item = u16;
}
impl<T> Show for Holder<T> {
    type Item = (T, u8);
}
impl<T: Copy> Show for Bounded<T> {
    type Item = T;
}
pub trait Other {
    type Item;
    type Out;
}
impl Other for Leaf {
    type Item = i64;
    type Out = i8;
}
pub trait Convert<T> {
    type Out;
}
impl Convert<u8> for Leaf {
    type Out = u32;
}
pub trait Defaulted<T = u8> {
    type Out;
}
impl Defaulted for Leaf {
    type Out = u32;
}
impl Defaulted<u16> for Leaf {
    type Out = u64;
}
// 比べる相手（`*_plain`）は同じ trait の impl に置く。impl の実装 trait も比較に入る
pub trait Sub: Show {
    fn shown(value: Self::Item) -> u8;
    fn shown_plain(value: u16) -> u8;
    fn other_plain(value: i64) -> u8;
    fn qualified(value: <Self as Show>::Item) -> u8;
}
impl Sub for Leaf {
    fn shown(value: Self::Item) -> u8 { value as u8 }
    fn shown_plain(value: u16) -> u8 { value as u8 }
    fn other_plain(value: i64) -> u8 { value as u8 }
    fn qualified(value: <Self as Show>::Item) -> u8 { value as u8 }
}
// Why: 閉包に読めない std の trait（`Clone`）が混じっても、所有 trait へ届けば選ぶ
pub trait Cloned: Show + Clone {
    fn cloned(value: Self::Item) -> u8;
    fn cloned_plain(value: u16) -> u8;
}
impl Cloned for Leaf {
    fn cloned(value: Self::Item) -> u8 { value as u8 }
    fn cloned_plain(value: u16) -> u8 { value as u8 }
}
// Why: 所有 trait（`Iterator`）が `const trait` で読めない。所有 trait を照合できず選ばない
impl Iterator for Counter {
    type Item = u16;
    fn next(&mut self) -> Option<u16> { None }
}
pub trait Counted: Iterator {
    fn counted(value: Self::Item) -> u8;
}
impl Counted for Counter {
    fn counted(value: Self::Item) -> u8 { value as u8 }
}
// 関連型 RHS の中の短縮形も、RHS を書いた impl から supertrait の impl を選ぶ
pub trait Rhs: Show {
    type Out;
    fn rhs(value: Self::Out) -> u8;
    fn rhs_plain(value: (u16, u8)) -> u8;
}
impl Rhs for Leaf {
    type Out = (Self::Item, u8);
    fn rhs(value: Self::Out) -> u8 { value.1 }
    fn rhs_plain(value: (u16, u8)) -> u8 { value.1 }
}
pub trait Held: Show {
    fn held(value: Self::Item) -> u8;
    fn held_plain(value: Self::Item) -> u8;
}
impl<T> Held for Holder<T> {
    fn held(value: Self::Item) -> u8 { value.1 }
    fn held_plain(value: (T, u8)) -> u8 { value.1 }
}
// Why: supertrait の impl が境界を持つので、境界の充足を確かめずに選ばない
pub trait Guarded: Show {
    fn guarded(value: Self::Item) -> u8;
}
impl<T: Copy> Guarded for Bounded<T> {
    fn guarded(value: Self::Item) -> u8 { let _ = value; 0 }
}
pub trait Deeper: Sub {
    fn chained(value: Self::Item) -> u8;
    fn chained_plain(value: u16) -> u8;
}
impl Deeper for Leaf {
    fn chained(value: Self::Item) -> u8 { value as u8 }
    fn chained_plain(value: u16) -> u8 { value as u8 }
}
pub trait Viewed
where
    Self: Show,
{
    fn viewed(value: Self::Item) -> u8;
    fn viewed_plain(value: u16) -> u8;
}
impl Viewed for Leaf {
    fn viewed(value: Self::Item) -> u8 { value as u8 }
    fn viewed_plain(value: u16) -> u8 { value as u8 }
}
pub trait Left: Show {}
pub trait Right: Show {}
pub trait Diamond: Left + Right {
    fn diamond(value: Self::Item) -> u8;
    fn diamond_plain(value: u16) -> u8;
}
impl Left for Leaf {}
impl Right for Leaf {}
impl Diamond for Leaf {
    fn diamond(value: Self::Item) -> u8 { value as u8 }
    fn diamond_plain(value: u16) -> u8 { value as u8 }
}
pub trait Mixed: Show + Other {
    fn mixed(value: Self::Out) -> u8;
    fn mixed_plain(value: i8) -> u8;
}
impl Mixed for Leaf {
    fn mixed(value: Self::Out) -> u8 { value as u8 }
    fn mixed_plain(value: i8) -> u8 { value as u8 }
}
pub trait Typed: Convert<u8> {
    fn typed(value: Self::Out) -> u8;
}
impl Typed for Leaf {
    fn typed(value: Self::Out) -> u8 { value as u8 }
}
// Why: 型引数を書いた supertrait は、既定の型引数の impl（`Defaulted<u8>`）と形が揃ってしまう
pub trait Picky: Defaulted<u16> {
    fn picky(value: Self::Out) -> u8;
}
impl Picky for Leaf {
    fn picky(value: Self::Out) -> u8 { value as u8 }
}
// 既定の型引数の supertrait を型引数なしで書くと、型引数なしで書いた impl（`Defaulted<u8>`）を選ぶ
pub trait Plain: Defaulted {
    fn plain(value: Self::Out) -> u8;
    fn plain_plain(value: u32) -> u8;
}
impl Plain for Leaf {
    fn plain(value: Self::Out) -> u8 { value as u8 }
    fn plain_plain(value: u32) -> u8 { value as u8 }
}
// Why: 同じ trait に型引数なしと型引数付きの 2 通りで届く。rustc は E0221 で拒否する（意図した形）
pub trait Mid: Defaulted<u16> {}
pub trait TwoPaths: Defaulted + Mid {
    fn two_paths(value: Self::Out) -> u8;
}
impl Mid for Leaf {}
impl TwoPaths for Leaf {
    fn two_paths(value: Self::Out) -> u8 { let _ = value; 0 }
}
// Why: 同名の関連型を持つ supertrait が 2 つある。rustc は E0221 で拒否する（意図した形）
pub trait Both: Show + Other {
    fn both(value: Self::Item) -> u8;
}
impl Both for Leaf {
    fn both(value: Self::Item) -> u8 { let _ = value; 0 }
}
impl far::FarSub for Leaf {
    fn distant(value: Self::Item) -> u8 { value as u8 }
    fn distant_plain(value: u16) -> u8 { value as u8 }
}
