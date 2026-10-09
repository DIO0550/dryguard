pub struct Holder<T>(pub T);
pub trait Project<T> {
    type Item;
    type Wrap<U>;
    fn short(value: Self::Item) -> Self::Item;
    fn qualified(value: <Self as Project<T>>::Item) -> <Self as Project<T>>::Item;
    fn plain(value: T) -> T;
    fn wrapped<U>(value: Self::Wrap<U>) -> <Self as Project<T>>::Wrap<U>;
    fn wrap_plain<U>(value: (T, U)) -> (T, U);
    fn mismatch<U>(value: <Self as Project<U>>::Item) -> <Self as Project<U>>::Item where Self: Project<U>;
    fn foreign(value: <Self as Other>::Item) -> <Self as Other>::Item where Self: Other;
    fn foreign_plain(value: u64) -> u64;
}
impl<T> Project<T> for Holder<T> {
    type Item = T;
    type Wrap<U> = (T, U);
    fn short(value: Self::Item) -> Self::Item { value }
    fn qualified(value: < Self /* scope */ as Project < T > > :: Item) -> <Self as Project<T>>::Item { value }
    fn plain(value: T) -> T { value }
    fn wrapped<U>(value: Self::Wrap<U>) -> <Self as Project<T>>::Wrap<U> { value }
    fn wrap_plain<U>(value: (T, U)) -> (T, U) { value }
    fn mismatch<U>(value: <Self as Project<U>>::Item) -> <Self as Project<U>>::Item where Self: Project<U> { value }
    fn foreign(value: <Self as Other>::Item) -> <Self as Other>::Item { value }
    fn foreign_plain(value: u64) -> u64 { value }
}
pub mod model {
    pub struct Twin;
}
pub trait Named {
    type Item;
    fn named(value: Self::Item) -> Self::Item;
    fn named_plain(value: model::Twin) -> model::Twin;
}
impl<T> Named for Holder<T> {
    type Item = model::Twin;
    fn named(value: Self::Item) -> Self::Item { value }
    fn named_plain(value: model::Twin) -> model::Twin { value }
}
pub trait Other {
    type Item;
    fn other(value: Self::Item) -> Self::Item;
}
impl<T> Other for Holder<T> {
    type Item = u64;
    fn other(value: Self::Item) -> Self::Item { value }
}
impl<T> Holder<T> {
    pub fn outside(value: <Self as Other>::Item) -> u64 { value }
    pub fn outside_plain(value: u64) -> u64 { value }
}
pub trait Limited {
    type Item;
    type Borrow<'a> where Self: 'a;
    type Pair<U>;
    type Again;
    fn itself(value: Self::Item) -> Self::Item;
    fn itself_plain(value: Self) -> Self;
    fn paired<U>(value: Self::Pair<U>) -> Self::Pair<U>;
    fn paired_plain<U>(value: (Self, U)) -> (Self, U) where Self: Sized;
    fn again(value: Self::Again) -> Self::Again;
    fn borrowed<'a>(value: Self::Borrow<'a>) -> Self::Borrow<'a> where Self: 'a;
}
impl<T> Limited for Holder<T> {
    type Item = Self;
    type Borrow<'a> = &'a T where Self: 'a;
    type Pair<U> = (Self, U);
    type Again = Self::Item;
    fn itself(value: Self::Item) -> Self::Item { value }
    fn itself_plain(value: Holder<T>) -> Holder<T> { value }
    fn paired<U>(value: Self::Pair<U>) -> Self::Pair<U> { value }
    fn paired_plain<U>(value: (Holder<T>, U)) -> (Holder<T>, U) { value }
    fn again(value: Self::Again) -> Self::Again { value }
    fn borrowed<'a>(value: Self::Borrow<'a>) -> Self::Borrow<'a> where Self: 'a { value }
}
