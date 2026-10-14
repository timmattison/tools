//! `wdid` — "what do i do": count which tools of this workspace the atuin
//! history uses.
//!
//! A workspace of many small tools makes one question hard to answer: which of
//! these tools does the person really use? The shell history holds the answer.
//! `wdid` reads that history through atuin, and it counts each tool.
//!
//! # The modules
//! - [`history`] gets the full atuin history as a list of rows.
