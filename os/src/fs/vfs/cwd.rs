use alloc::string::String;
use alloc::vec::Vec;

/// Converts any path into a clean, absolute path.
/// 
/// - `cwd`: Current Working Directory. It must be an absolute path. 
///          If `path` is already absolute, `cwd` will be ignored.
/// - `path`: The target path input by the user. It can be absolute or relative.
pub fn build_absolute_path(cwd: &str, path: &str) -> String {
    let mut stack = Vec::new();
    // If it is a relative path, push all parts of `cwd` into the stack first.
    if !path.starts_with('/') {
        for part in cwd.split('/').filter(|s| !s.is_empty()) {
            stack.push(part);
        }
    }
    //
    for part in path.split('/').filter(|s| !s.is_empty()) {
        match part {
            "." => {
            }
            ".." => {
                stack.pop();
            }
            _ => {
                // Normal directory or file: add it to the stack
                stack.push(part);
            }
        }
    }
    // Rebuild the final absolute path from the stack.
    if stack.is_empty() {
        String::from("/")
    } else {
        let mut abs_path = String::from("/");
        abs_path.push_str(&stack.join("/"));
        abs_path
    }
}