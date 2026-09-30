//! Arc — memoria (Fase R1, ADR-0040).
//!
//! Frame allocator fisico (`phys_mem`), VM del kernel (`vmm`: higher-half +
//! direct map), address space user (`vmm_user`) e heap del kernel (`heap`).
//! I chiamanti usano i path `crate::arc::*`.

pub mod heap;
pub mod phys_mem;
pub mod vmm;
pub mod vmm_user;
