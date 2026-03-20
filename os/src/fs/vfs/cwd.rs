use alloc::string::String;
use alloc::vec::Vec;

/// 将任意路径转换为干净的绝对路径
/// - `cwd`: 当前工作目录 (Current Working Directory)，必须是绝对路径。如果传的是绝对路径，cwd 会被忽略。
/// - `path`: 用户输入的路径，可能是绝对的，也可能是相对的。
pub fn build_absolute_path(cwd: &str, path: &str) -> String {
    let mut stack = Vec::new();

    // 1. 如果不是以 '/' 开头，说明是相对路径，先把 cwd 的每一层压入栈中
    if !path.starts_with('/') {
        for part in cwd.split('/').filter(|s| !s.is_empty()) {
            stack.push(part);
        }
    }

    // 2. 遍历用户输入的 path，进行压栈和出栈操作
    for part in path.split('/').filter(|s| !s.is_empty()) {
        match part {
            "." => {
                // 当前目录，什么都不做
            }
            ".." => {
                // 上一级目录，把栈顶元素弹出来（如果栈不为空）
                stack.pop();
            }
            _ => {
                // 普通目录或文件，压入栈中
                stack.push(part);
            }
        }
    }

    // 3. 将栈里的元素重新拼装成绝对路径
    if stack.is_empty() {
        String::from("/")
    } else {
        let mut abs_path = String::from("/");
        abs_path.push_str(&stack.join("/"));
        abs_path
    }
}