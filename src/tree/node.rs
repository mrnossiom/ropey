use std::sync::Arc;

use super::{Children, Text, TextInfo, MAX_CHILDREN};

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Internal(Arc<Children>),
    Leaf(Arc<Text>),
}

impl Node {
    /// Shallowly computes the text info of this node.
    ///
    /// Assumes that the info of this node's children is up to date.
    pub(crate) fn text_info(&self) -> TextInfo {
        match &self {
            Node::Internal(children) => {
                let mut acc_info = TextInfo::new();
                for info in children.info() {
                    acc_info = acc_info.append(*info);
                }
                acc_info
            }
            Node::Leaf(text) => text.text_info(),
        }
    }

    #[inline(always)]
    pub(crate) fn is_internal(&self) -> bool {
        match self {
            &Self::Internal(_) => true,
            &Self::Leaf(_) => false,
        }
    }

    #[inline(always)]
    pub(crate) fn is_leaf(&self) -> bool {
        match self {
            &Self::Internal(_) => false,
            &Self::Leaf(_) => true,
        }
    }

    pub fn child_count(&self) -> usize {
        self.children().len()
    }

    pub fn children(&self) -> &Children {
        match *self {
            Node::Internal(ref children) => children,
            _ => panic!(),
        }
    }

    pub fn children_mut(&mut self) -> &mut Children {
        match *self {
            Node::Internal(ref mut children) => Arc::make_mut(children),
            _ => panic!(),
        }
    }

    pub fn leaf_text(&self) -> [&str; 2] {
        match *self {
            Node::Leaf(ref text) => text.chunks(),
            _ => panic!(),
        }
    }

    pub fn leaf_text_mut(&mut self) -> &mut Text {
        match *self {
            Node::Leaf(ref mut text) => Arc::make_mut(text),
            _ => panic!(),
        }
    }

    /// Note: `node_info` is the text info *for the node this is being called
    /// on*.  This is because node info for a child is stored in the parent.
    /// This makes it a little inconvenient to call, but is desireable for
    /// efficiency so that the info can be used for a cheaper update rather than
    /// being recomputed from scratch.
    ///
    ///
    /// On success, returns the new text info for the current node, and if a
    /// split was caused returns the right side of the split (the left remaining
    /// as the current node) and its text info.
    ///
    /// On non-panicing failure, returns Err(()).  This happens if and only if
    /// `byte_idx` is not on a char boundary.
    ///
    /// Panics:
    /// - If `byte_idx` is out of bounds.
    /// - If `text` is too large to handle.  Anything less than or equal to
    ///   `MAX_TEXT_SIZE - 4` is guaranteed to be okay.
    pub fn insert_at_byte_idx(
        &mut self,
        byte_idx: usize,
        text: &str,
        _node_info: TextInfo,
    ) -> Result<(TextInfo, Option<(TextInfo, Node)>), ()> {
        // TODO: use `node_info` to do an update of the node info rather
        // than recomputing from scratch.  This will be a bit delicate,
        // because it requires being aware of crlf splits.

        match *self {
            Node::Leaf(ref mut leaf_text) => {
                if !leaf_text.is_char_boundary(byte_idx) {
                    // Not a char boundary, so early-out.
                    return Err(());
                }

                let leaf_text = Arc::make_mut(leaf_text);
                if text.len() <= leaf_text.free_capacity() {
                    // Enough room to insert.
                    leaf_text.insert_str(byte_idx, text);
                    Ok((leaf_text.text_info(), None))
                } else {
                    // Not enough room to insert.  Need to split into two nodes.
                    let mut right_text = leaf_text.split(byte_idx);
                    let text_split_idx =
                        crate::find_split_l(leaf_text.free_capacity(), text.as_bytes());
                    leaf_text.append_str(&text[..text_split_idx]);
                    right_text.insert_str(0, &text[text_split_idx..]);
                    leaf_text.distribute(&mut right_text);
                    Ok((
                        leaf_text.text_info(),
                        Some((right_text.text_info(), Node::Leaf(Arc::new(right_text)))),
                    ))
                }
            }
            Node::Internal(ref mut children) => {
                let children = Arc::make_mut(children);

                // Find the child we care about.
                let (child_i, acc_byte_idx) = children.search_byte_idx_only(byte_idx);
                let info = children.info()[child_i];

                // Recurse into the child.
                let (l_info, residual) = children.nodes_mut()[child_i].insert_at_byte_idx(
                    byte_idx - acc_byte_idx,
                    text,
                    info,
                )?;
                children.info_mut()[child_i] = l_info;

                // Handle the residual node if there is one and return.
                if let Some((r_info, r_node)) = residual {
                    if children.len() < MAX_CHILDREN {
                        children.insert(child_i + 1, (r_info, r_node));
                        Ok((children.combined_text_info(), None))
                    } else {
                        let r = children.insert_split(child_i + 1, (r_info, r_node));
                        let r_info = r.combined_text_info();
                        Ok((
                            children.combined_text_info(),
                            Some((r_info, Node::Internal(Arc::new(r)))),
                        ))
                    }
                } else {
                    Ok((children.combined_text_info(), None))
                }
            }
        }
    }

    pub fn remove_byte_range(
        &mut self,
        byte_idx_range: [usize; 2],
        _node_info: TextInfo,
    ) -> Result<TextInfo, ()> {
        // TODO: use `node_info` to do an update of the node info rather
        // than recomputing from scratch.  This will be a bit delicate,
        // because it requires being aware of crlf splits.

        match *self {
            Node::Leaf(ref mut leaf_text) => {
                debug_assert!(byte_idx_range[0] > 0 || byte_idx_range[1] < leaf_text.len());
                if byte_idx_range
                    .iter()
                    .any(|&i| !leaf_text.is_char_boundary(i))
                {
                    // Not a char boundary, so early-out.
                    return Err(());
                }

                let leaf_text = Arc::make_mut(leaf_text);
                leaf_text.remove(byte_idx_range);

                Ok(leaf_text.text_info())
            }
            Node::Internal(ref mut children) => {
                let children = Arc::make_mut(children);

                // Find the start and end children of the range, and
                // their left-side byte indices within this node.
                let (start_child_i, start_child_left_byte_idx) =
                    children.search_byte_idx_only(byte_idx_range[0]);
                let (end_child_i, end_child_left_byte_idx) =
                    children.search_byte_idx_only(byte_idx_range[1]);

                // Text info the the start and end children.
                let start_info = children.info()[start_child_i];
                let end_info = children.info()[end_child_i];

                // Compute the start index relative to the contents of the
                // first child, and the end index relative to the contents
                // of the second.
                let start_byte_idx = byte_idx_range[0] - start_child_left_byte_idx;
                let end_byte_idx = byte_idx_range[1] - end_child_left_byte_idx;

                // Simple case: the removal is entirely within a single child.
                if start_child_i == end_child_i {
                    if start_byte_idx == 0 && end_byte_idx == start_info.bytes as usize {
                        children.remove(start_child_i);
                    } else {
                        let new_info = children.nodes_mut()[start_child_i]
                            .remove_byte_range([start_byte_idx, end_byte_idx], start_info)?;
                        children.info_mut()[start_child_i] = new_info;
                    }
                    Ok(children.combined_text_info())
                }
                // More complex case: the removal spans multiple children.
                else {
                    let remove_whole_start_child = start_byte_idx == 0;
                    let remove_whole_end_child =
                        end_byte_idx == children.info()[end_child_i].bytes as usize;

                    // Handle partial removal of leftmost child.
                    if !remove_whole_start_child {
                        let new_info = children.nodes_mut()[start_child_i].remove_byte_range(
                            [start_byte_idx, start_info.bytes as usize],
                            start_info,
                        )?;
                        children.info_mut()[start_child_i] = new_info;
                    }

                    // Handle partial removal of rightmost child.
                    if !remove_whole_end_child {
                        let new_info = children.nodes_mut()[end_child_i]
                            .remove_byte_range([0, end_byte_idx], end_info)?;
                        children.info_mut()[end_child_i] = new_info;
                    }

                    // Remove nodes that need to be completely removed.
                    {
                        let removal_start = if remove_whole_start_child {
                            start_child_i
                        } else {
                            start_child_i + 1
                        };
                        let removal_end = if remove_whole_end_child {
                            end_child_i + 1
                        } else {
                            end_child_i
                        };

                        if removal_start < removal_end {
                            children.remove_multiple([removal_start, removal_end]);
                        }
                    }

                    Ok(children.combined_text_info())
                }
            }
        }
    }

    //---------------------------------------------------------
    // Debugging helpers.

    /// Checks that all leaf nodes are at the same depth.
    pub fn assert_equal_leaf_depth(&self) -> usize {
        match *self {
            Node::Leaf(_) => 1,
            Node::Internal(ref children) => {
                let first_depth = children.nodes()[0].assert_equal_leaf_depth();
                for node in &children.nodes()[1..] {
                    assert_eq!(node.assert_equal_leaf_depth(), first_depth);
                }
                first_depth + 1
            }
        }
    }

    /// Checks that there are no empty internal nodes in the tree.
    pub fn assert_no_empty_internal(&self) {
        match *self {
            Node::Leaf(_) => {}
            Node::Internal(ref children) => {
                assert!(children.len() > 0);
                for node in children.nodes() {
                    node.assert_no_empty_internal();
                }
            }
        }
    }

    /// Checks that there are no empty internal nodes in the tree.
    pub fn assert_no_empty_leaf(&self) {
        match *self {
            Node::Leaf(ref text) => {
                assert!(text.len() > 0);
            }
            Node::Internal(ref children) => {
                for node in children.nodes() {
                    node.assert_no_empty_leaf();
                }
            }
        }
    }

    /// Checks that all cached TextInfo in the tree is correct.
    pub fn assert_accurate_text_info(&self) -> TextInfo {
        match *self {
            Node::Leaf(ref text) => {
                // Freshly compute the relevant info from scratch.
                let info_l = TextInfo::from_str(text.chunks()[0]);
                let info_r = TextInfo::from_str(text.chunks()[1]);
                let info = info_l.append(info_r);

                // Make sure everything matches.
                assert_eq!(text.text_info(), info);
                assert_eq!(text.left_info, info_l);

                info
            }
            Node::Internal(ref children) => {
                let mut acc_info = TextInfo::new();
                for (node, &info) in children.nodes().iter().zip(children.info().iter()) {
                    assert_eq!(info, node.assert_accurate_text_info());
                    acc_info = acc_info.append(info);
                }

                acc_info
            }
        }
    }
}
