pub type Pair<T> = (T, T);
pub type DefaultPair<T = u8, U = T> = (T, U);
pub type Bounded<T: Copy = u8> = (T, T);
pub type Borrowed<'a, T> = &'a T;
pub type Array<T, const N: usize> = [T; N];

pub type Flip<A, B> = (B, A);
pub type PrimitiveParameter<u8> = (u8, u8);
pub type NestedDefault<T, U = (T, T)> = (T, U);

pub fn pair(value: Pair<u8>) -> Pair<u8> { value }
pub fn tuple(value: (u8, u8)) -> (u8, u8) { value }
pub fn large(value: Pair<u64>) -> Pair<u64> { value }
pub fn nested(value: Pair<Pair<u8>>) -> Pair<Pair<u8>> { value }
pub fn nested_tuple(value: ((u8, u8), (u8, u8))) -> ((u8, u8), (u8, u8)) { value }
pub fn defaulted(value: DefaultPair) -> DefaultPair { value }
pub fn partial(value: DefaultPair<u64>) -> DefaultPair<u64> { value }
pub fn bounded(value: Bounded) -> Bounded { value }
pub fn flipped<T, U>(value: Flip<T, U>, other: T) -> T { other }
pub fn flipped_tuple<A, B>(value: (B, A), other: A) -> A { other }
pub fn primitive_parameter(value: PrimitiveParameter<u8>) -> PrimitiveParameter<u8> { value }
pub fn nested_default(value: NestedDefault<u8>) -> NestedDefault<u8> { value }
pub fn default_tuple(value: (u8, (u8, u8))) -> (u8, (u8, u8)) { value }
pub fn lifetime(value: Borrowed<'_, u8>) -> Borrowed<'_, u8> { value }
pub fn constant(value: Array<u8, 3>) -> Array<u8, 3> { value }
pub fn qualified(value: crate::generic::Pair<u8>) -> crate::generic::Pair<u8> { value }
pub fn named(value: Pair<crate::model::Customer>) -> Pair<crate::model::Customer> { value }
pub fn named_tuple(value: (crate::model::Customer, crate::model::Customer)) -> (crate::model::Customer, crate::model::Customer) { value }

pub mod shadowed {
    type u8 = u64;
    pub type Default<T = u8> = (T, T);
    pub fn default_shadow(value: Default) -> Default { value }
}
