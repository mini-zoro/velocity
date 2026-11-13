# ST 行为待核实清单

本轮只读取 [官方公开 API 文档](https://www.sublimetext.com/docs/api_reference.html)，没有对真实编辑器录制 golden。
下面每一项在实现处都有 `// ST-BEHAVIOR: unverified [ID]`；它们是本轮选定的可测试行为，
不是对 Sublime Text 已观测事实的声明。字节坐标、类型适配、限额等 Rust 接口选择见 README。

| ID | 位置 | 本轮行为，待黑盒 golden 核实 |
|---|---|---|
| R1 | `region.rs` | contains 包含两端；非空 intersects 要共享非空区间，空 caret 可与任一边界相交；cover/intersection 向前；无交集为 `(0,0)`，接触交集为空的接触点。 |
| S1 | `region.rs` | 重叠与接触的非空区域归并，边界 caret 也归并；最后交互添加者的方向及目标列获胜，primary 保持独立于排序。 |
| S2 | `region.rs` | subtract 分裂时保留原方向，删除所有区域后在 cut.begin 留 caret；空 cut 只移除同位置空 caret。 |
| S3 | `region.rs` | 空集合 normalize 后补零点 caret；new/clear 可以临时为空，安装到视图时必规范化。 |
| M1 | `change.rs` | named-region begin 右黏、end 左黏；空区域按右黏 caret；端点逆转时坍缩为 begin 处的空区域；drop_empty 决定是否丢弃。 |
| M2 | `buffer.rs` | 注释通过 undo/redo 的变更图映射，不恢复已 drop 的项；相同 begin 的排序保持输入次序。 |
| H1 | `buffer.rs` | 空/无内容变化事务仍生成修订与 undo 项，dirty 依保存修订身份，而非每次比较全文。 |
| H2 | `buffer.rs::can_group` | word 与随后空白成组，已有 word 后的下个 word 开组；前导空白可跟首 word 成组；标点/LF 类切换断组；所有外部选区改变断组。 |
| H3 | `buffer.rs` | 普通 undo/redo 是 buffer-global，恢复原编辑视图的选区；soft undo 是每视图事件序列，文本事件仅在当前修订相容时执行，选区事件可按日志映射。 |
| H4 | `buffer.rs` | 限额按事务数记账，淘汰完整最旧组；mark_saved 断组；过时软事件跳过，分支编辑清空 redo。 |
| V1 | `layout.rs` | 无界面布局选最近 grapheme 边界，等距向左；非有限目标取零列；tab 按 stop、东亚宽字为两列，纵向边界夹到首尾行。 |
| W1 | `ops.rs` | Words 停字符类边界；WordEnds/SubwordEnds 先跳过横向空白；LF 独立停点；Subwords 在小写→大写和 `_` 两侧停；自定义 Stops 无命中时到首尾。 |
| W2 | `ops.rs` | word expansion / caret 查找在 EOF 取前一簇，标点与空白按各自完整类 run 展开，LF 自成区域。 |
| W3 | `ops.rs` | Bol 为零列，HardBol 在首个非空格/tab 与零列间切换；未引入视觉换行时 Eol/HardEol 相同，均在 LF 前。 |
| E1 | `ops.rs::paste` | 只按 LF 分配，保留末尾空字段及 CR；只有多选区且字段数恰等于选区数才逐行分配。 |
| E2 | `ops.rs` | 命令先去除替换中不变的 scalar 前后缀，保留未改变的行内位置；文本变换后的非空选区覆盖其替换结果；替换内部 caret 坍缩至替换起点，起始边界 caret 右黏。 |
| F1 | `ops.rs` | 查找是区分大小写、非重叠的字面匹配；空 caret 启用 whole-word 后续匹配，手动非空选区不启用；从 primary/缓存末端继续、到末尾后回绕一次、跳过已有完整匹配。 |
| L1 | `ops.rs` / `region.rs` | 非空选区恰好结束于下一 BOL 时不算触及下一行，LF 归属上一行；各行只编辑一次。 |
| L2 | `ops.rs::expand_selection` | Indentation 包含相邻缩进不小于 seed 的行及空行；缩进按前导空格/tab 的字节数比较，tab 计一。 |
| L3 | `ops.rs::split_selection_into_lines` | LF 不属于拆分片段，空行保留，结束于 BOL 不额外加 caret；片段保留方向，最后生成片段为 primary。 |
| L4 | `ops.rs::trim_trailing_whitespace` | 清理 Unicode whitespace，包括 literal CR，保留 LF 行结构；仅触及的行。 |
| L5 | `ops.rs::{sort_lines,reverse_lines}` | 连续触及的行构成独立块，保留尾 LF；排序按 Unicode scalar 字典序，保留重复行。 |
| L6 | `ops.rs::join_lines` | 单行 seed 与后继拼接，多行 seed 只拼选中块内部；删去接缝两侧空格/tab，插一个 ASCII 空格；空行也产生一个空格。 |
| L7 | `ops.rs::duplicate_line` | 每个触及行只复制一次，无尾 LF 的最后行在两份间补 LF；选区按插入偏置跟随。 |
| L8 | `ops.rs::swap_line_*` | 连续块移动一行，边缘块不动，保留总行终止形式、caret 行内字节列及选区方向。 |
| C1 | `ops.rs` | 空 caret 对其 word run 做大小写变换；title 使用 word classes 定首字母；Unicode scalar 大小写可改变长度，无 locale。 |
| C2 | `ops.rs::transpose` | caret 交换相邻 grapheme，BOL/EOF 用首/末一对；非空区域逆转 grapheme，重叠请求先归并；ZWJ/组合簇内部不拆。 |

公开文档有明确说明的 API 名称与属性仍以其为准；后续 golden 若否定以上决策，应先改集中策略与测试，
不读取应用内的 Python 实现、Default 包或二进制来确认。
