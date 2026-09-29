//! Push credential references and ephemeral private-key material.

mod material;
mod reference;
mod resolve;

pub use material::{Material, ssh_command};
pub use reference::{Ref, SHAPES, parse};
pub use resolve::{
    ErrWorker, KEYCHAIN_SERVICE, RunError, Runner, Secret, TOKEN_ENV, keychain_bytes, new, op_env,
    redact, token_file_name,
};
pub use resolve::{Resolver, exec, first_line, is_private_key};
