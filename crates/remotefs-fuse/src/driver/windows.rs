#[cfg(test)]
mod test;

use super::Driver;

use dokan::FileSystemHandler;
use remotefs::{File, RemoteFs};

// For reference <https://github.com/dokan-dev/dokan-rust/blob/master/dokan/examples/memfs/main.rs>
impl<'c, 'h: 'c, T> FileSystemHandler<'c, 'h> for Driver<T>
where
    T: RemoteFs + Sync + 'h,
{
    type Context = File;
}
