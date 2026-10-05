pub mod declaration {
    pub struct Customer;
    pub type Alias = Customer;
    pub type Chain = Alias;
    pub type Pair = (Alias, Customer);
    pub type NamedGeneric = Option<Customer>;
    pub type Instantiated = super::super::model::Pair<Customer>;
    pub fn direct(value: Customer) -> Customer {
        value
    }
    pub fn aliased(value: Alias) -> Alias {
        value
    }
    pub fn chained(value: Chain) -> Chain {
        value
    }
    pub fn paired(value: Pair) -> Pair {
        value
    }
    pub fn pair_raw(value: (Customer, Customer)) -> (Customer, Customer) {
        value
    }
    pub fn option(value: NamedGeneric) -> NamedGeneric {
        value
    }
    pub fn option_raw(value: Option<Customer>) -> Option<Customer> {
        value
    }
    pub fn instantiated(value: Instantiated) -> Instantiated {
        value
    }
}

pub mod usage {
    pub struct Customer;
    pub use super::declaration::Alias as Imported;
    pub fn imported(value: Imported) -> Imported {
        value
    }
    pub fn wrong(value: Customer) -> Customer {
        value
    }
    pub fn captured<Customer>(value: Imported, other: Customer) -> Imported {
        value
    }
    pub fn explicit<T>(
        value: super::declaration::Customer,
        other: T,
    ) -> super::declaration::Customer {
        value
    }
}

pub mod other {
    pub struct Customer;
    pub type Alias = Customer;
    pub fn other_aliased(value: Alias) -> Alias {
        value
    }
    pub fn other_option(value: Option<Customer>) -> Option<Customer> {
        value
    }
}

pub mod cycles {
    pub type First = Second;
    pub type Second = First;
    pub fn cyclic(value: First) -> First {
        value
    }
}

pub mod constants {
    pub mod a {
        pub const COUNT: usize = 3;
        pub type Bytes = [u8; COUNT];
        pub fn bytes_a(value: Bytes) -> Bytes {
            value
        }
    }
    pub mod b {
        pub const COUNT: usize = 7;
        pub type Bytes = [u8; COUNT];
        pub fn bytes_b(value: Bytes) -> Bytes {
            value
        }
    }
}

pub mod depth {
    pub type A0 = A1;
    pub type A1 = A2;
    pub type A2 = A3;
    pub type A3 = A4;
    pub type A4 = A5;
    pub type A5 = A6;
    pub type A6 = A7;
    pub type A7 = A8;
    pub type A8 = A9;
    pub type A9 = A10;
    pub type A10 = A11;
    pub type A11 = A12;
    pub type A12 = A13;
    pub type A13 = A14;
    pub type A14 = A15;
    pub type A15 = A16;
    pub type A16 = A17;
    pub type A17 = A18;
    pub type A18 = A19;
    pub type A19 = A20;
    pub type A20 = A21;
    pub type A21 = A22;
    pub type A22 = A23;
    pub type A23 = A24;
    pub type A24 = A25;
    pub type A25 = A26;
    pub type A26 = A27;
    pub type A27 = A28;
    pub type A28 = A29;
    pub type A29 = A30;
    pub type A30 = A31;
    pub type A31 = A32;
    pub type A32 = A33;
    pub type A33 = A34;
    pub type A34 = u8;
    pub fn limit(value: A0) -> A0 {
        value
    }
    pub fn boundary(value: A3) -> A3 {
        value
    }
    pub fn raw(value: u8) -> u8 {
        value
    }
}

use super::shadowed::Number as u64;
pub fn shadowed_direct(value: u64) -> u64 {
    value
}

pub mod cache_order {
    pub type Terminal<T> = (T, T);
    pub type B0 = B1;
    pub type B1 = B2;
    pub type B2 = B3;
    pub type B3 = B4;
    pub type B4 = B5;
    pub type B5 = B6;
    pub type B6 = B7;
    pub type B7 = B8;
    pub type B8 = B9;
    pub type B9 = B10;
    pub type B10 = B11;
    pub type B11 = B12;
    pub type B12 = B13;
    pub type B13 = B14;
    pub type B14 = B15;
    pub type B15 = B16;
    pub type B16 = B17;
    pub type B17 = B18;
    pub type B18 = B19;
    pub type B19 = B20;
    pub type B20 = B21;
    pub type B21 = B22;
    pub type B22 = B23;
    pub type B23 = B24;
    pub type B24 = B25;
    pub type B25 = B26;
    pub type B26 = B27;
    pub type B27 = B28;
    pub type B28 = B29;
    pub type B29 = B30;
    pub type B30 = B31;
    pub type B31 = Terminal<u8>;
    pub fn warm_limit(first: Terminal<u8>, second: B0) {}
    pub fn cold_limit(first: B0, second: Terminal<u8>) {}
}

pub mod expansion_size {
    pub type S0 = u8;
    pub type S1 = (S0, S0);
    pub type S2 = (S1, S1);
    pub type S3 = (S2, S2);
    pub type S4 = (S3, S3);
    pub type S5 = (S4, S4);
    pub type S6 = (S5, S5);
    pub type S7 = (S6, S6);
    pub type S8 = (S7, S7);
    pub type S9 = (S8, S8);
    pub type S10 = (S9, S9);
    pub type S11 = (S10, S10);
    pub type S12 = (S11, S11);
    pub type S13 = (S12, S12);
    pub type S14 = (S13, S13);
    pub type S15 = (S14, S14);
    pub type S16 = (S15, S15);
    pub fn size_limit(value: S16) -> S16 {
        value
    }
}

pub fn lifetime_nominal<'a>(value: std::borrow::Cow<'a, str>) -> std::borrow::Cow<'a, str> {
    value
}
pub fn lifetime_nominal_renamed<'b>(value: std::borrow::Cow<'b, str>) -> std::borrow::Cow<'b, str> {
    value
}
pub fn iterator_binding<I: Iterator<Item = u8>>(value: I) -> I {
    value
}
pub fn iterator_binding_renamed<J: Iterator<Item = u8>>(value: J) -> J {
    value
}
