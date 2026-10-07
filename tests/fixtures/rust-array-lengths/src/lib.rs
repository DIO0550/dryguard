mod counts;
pub use counts::{Bytes, COUNT as REEXPORTED};

pub const COUNT: usize = 5;
pub const CYCLE_A: usize = CYCLE_B;
pub const CYCLE_B: usize = CYCLE_A;
pub const TOO_BIG: usize = 65536;
pub type LocalBytes = [u8; COUNT];
pub type Chained = Bytes;
pub type Expression = [u8; { (counts::BASE + 2) * 2 / 2 }];

pub fn literal(a: [u8; 4]) -> [u8; 5] { [0; 5] }
pub fn arithmetic(a: [u8; 0x8 / 0b10]) -> [u8; (3 + 3) - 1] { [0; 5] }
pub fn imported(a: [u8; REEXPORTED]) -> [u8; COUNT] { [0; COUNT] }
pub fn aliased(a: Chained) -> LocalBytes { [0; COUNT] }
pub fn expression(a: Expression) -> LocalBytes { [0; COUNT] }
pub fn different(a: [u8; COUNT]) -> [u8; COUNT] { [0; COUNT] }
pub fn nested(a: ([u8; 2 + 2], [[u8; 1 + 1]; 3])) -> [u8; 4 % 3] { [0; 1] }
pub fn nested_plain(a: ([u8; 4], [[u8; 2]; 3])) -> [u8; 1] { [0; 1] }
pub fn unsupported(a: [u8; count()]) {}
pub fn cyclic(a: [u8; CYCLE_A]) {}
pub fn limited(a: [u8; TOO_BIG]) {}
pub fn zero_division(a: [u8; 4 / 0]) {}
pub fn negative(a: [u8; 2 - 3]) {}
pub fn generic<const N: usize>(a: [u8; N]) {}
const fn count() -> usize { 4 }

macro_rules! array { () => { [u8; 4] }; }
pub fn macro_type(a: array!()) {}
pub fn suffix(a: [u8; 0o4_usize]) -> [u8; 0b101] { [0; 5] }
pub fn maximum(a: [u8; 65535]) {}
pub fn large_intermediate(a: [u8; (65535 + 1) - 1]) {}
pub fn cast(a: [u8; 4u8 as usize]) {}
pub fn commented(a: [u8; { /*🦀*/ REEXPORTED }]) -> [u8; COUNT] { [0; COUNT] }
pub type UnsupportedAlias = [u8; count()];
pub fn unsupported_alias(a: UnsupportedAlias) {}
pub type CyclicAlias = [u8; CYCLE_A];
pub fn cyclic_alias(a: CyclicAlias) {}
const CHAIN0: usize = 1;
const CHAIN1: usize = CHAIN0;
const CHAIN2: usize = CHAIN1;
const CHAIN3: usize = CHAIN2;
const CHAIN4: usize = CHAIN3;
const CHAIN5: usize = CHAIN4;
const CHAIN6: usize = CHAIN5;
const CHAIN7: usize = CHAIN6;
const CHAIN8: usize = CHAIN7;
const CHAIN9: usize = CHAIN8;
const CHAIN10: usize = CHAIN9;
const CHAIN11: usize = CHAIN10;
const CHAIN12: usize = CHAIN11;
const CHAIN13: usize = CHAIN12;
const CHAIN14: usize = CHAIN13;
const CHAIN15: usize = CHAIN14;
const CHAIN16: usize = CHAIN15;
const CHAIN17: usize = CHAIN16;
const CHAIN18: usize = CHAIN17;
const CHAIN19: usize = CHAIN18;
const CHAIN20: usize = CHAIN19;
const CHAIN21: usize = CHAIN20;
const CHAIN22: usize = CHAIN21;
const CHAIN23: usize = CHAIN22;
const CHAIN24: usize = CHAIN23;
const CHAIN25: usize = CHAIN24;
const CHAIN26: usize = CHAIN25;
const CHAIN27: usize = CHAIN26;
const CHAIN28: usize = CHAIN27;
const CHAIN29: usize = CHAIN28;
const CHAIN30: usize = CHAIN29;
const CHAIN31: usize = CHAIN30;
const CHAIN32: usize = CHAIN31;
pub fn chain_boundary(a: [u8; CHAIN31]) {}
pub fn chain_limit(a: [u8; CHAIN32]) {}
const DAG0: usize = 0;
const DAG1: usize = DAG0 + DAG0;
const DAG2: usize = DAG1 + DAG1;
const DAG3: usize = DAG2 + DAG2;
const DAG4: usize = DAG3 + DAG3;
const DAG5: usize = DAG4 + DAG4;
const DAG6: usize = DAG5 + DAG5;
const DAG7: usize = DAG6 + DAG6;
const DAG8: usize = DAG7 + DAG7;
const DAG9: usize = DAG8 + DAG8;
const DAG10: usize = DAG9 + DAG9;
pub fn work_limit(a: [u8; DAG10]) {}
pub fn expression_limit(a: [u8; (((((((((((((((((((((((((((((((((4)))))))))))))))))))))))))))))))))]) {}

mod shadow {
    // Why: usize と綴られていても、遮蔽された型を配列長のプリミティブとして扱わない。
    type usize = u8;
    const BAD: usize = 4;
    pub fn shadowed(a: [u8; BAD]) {}
}

mod same_named {
    use super::counts::COUNT;
    pub fn scoped_count(a: [u8; COUNT]) -> [u8; 5] { [0; 5] }
}
pub fn nested_different(a: ([u8; 4], [[u8; 2]; 4])) -> [u8; 1] { [0; 1] }
