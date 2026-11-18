use vstd::prelude::*;

verus! {

pub struct R(pub u32);

pub trait A {
    spec fn a_spec(&self) -> bool;

    fn f(&self) -> u32;

    fn twice(&self) -> u32 {
        self.f()
    }
}

pub trait B {
    fn f(&self) -> u32;
}

impl A for R {
    spec fn a_spec(&self) -> bool {
        true
    }

    fn f(&self) -> u32 {
        1
    }
}

impl B for R {
    fn f(&self) -> u32 {
        2
    }
}

impl R {
    pub fn f(&self) -> u32 {
        3
    }
}

pub fn call_a(r: &R) -> u32 {
    <R as A>::f(r)
}

pub fn call_generic<T: A>(t: &T) -> u32 {
    t.f()
}

pub fn call_inherent(r: &R) -> u32 {
    r.f()
}

#[cfg(feature = "alt")]
pub fn alt() -> u32 {
    0
}

#[cfg(not(feature = "alt"))]
pub fn alt() -> u32 {
    call_inherent(&R(0))
}

} // verus!
