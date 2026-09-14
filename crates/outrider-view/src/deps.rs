//! Dependency-tracking bitset for incremental re-resolution.

/// Which session inputs a resolved artifact depends on.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Deps(u32);

impl Deps {
    pub const NONE: Deps = Deps(0);
    pub const TREE: Deps = Deps(1 << 0);
    pub const FOCUS: Deps = Deps(1 << 1);
    pub const CAMERA: Deps = Deps(1 << 2);
    pub const SELECTION: Deps = Deps(1 << 3);
    pub const HOVER: Deps = Deps(1 << 4);
    pub const GIT: Deps = Deps(1 << 5);
    pub const SPEC: Deps = Deps(1 << 6);
    pub const METRICS: Deps = Deps(1 << 7);
    pub const RELATIONS: Deps = Deps(1 << 8);

    pub const fn union(self, other: Deps) -> Deps {
        Deps(self.0 | other.0)
    }

    pub const fn intersects(self, other: Deps) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn contains(self, other: Deps) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// The bits of `self` that are not in `other`.
    pub const fn without(self, other: Deps) -> Deps {
        Deps(self.0 & !other.0)
    }
}

impl std::ops::BitOr for Deps {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Deps(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Deps {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for Deps {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Deps(self.0 & rhs.0)
    }
}
