mod common;

use common::write_temp;
use fastlogviewer_core::{CancelToken, LogFile};

#[test]
fn copy_line_returns_line_bytes_without_terminator() {
    let path = write_temp("alpha\r\nbeta\ngamma");
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        let mut buf = [0u8; 64];

        assert_eq!(5, f.copy_line(0, &mut buf));
        assert_eq!("alpha", std::str::from_utf8(&buf[..5]).unwrap());
        assert_eq!(4, f.copy_line(1, &mut buf));
        assert_eq!("beta", std::str::from_utf8(&buf[..4]).unwrap());
        assert_eq!(5, f.copy_line(2, &mut buf));
        assert_eq!("gamma", std::str::from_utf8(&buf[..5]).unwrap());
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn copy_line_truncates_to_buffer_and_clamps_out_of_range() {
    let path = write_temp("abcdefghij\n");
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        let mut small = [0u8; 4];
        assert_eq!(4, f.copy_line(0, &mut small));
        assert_eq!("abcd", std::str::from_utf8(&small[..4]).unwrap());
        assert_eq!(0, f.copy_line(99, &mut small));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn get_line_drops_the_terminator_on_the_last_line() {
    for content in ["alpha\nomega\n", "alpha\r\nomega\r\n"] {
        let path = write_temp(content);
        {
            let f = LogFile::open(&path).unwrap();
            f.build_index(&CancelToken::none());
            assert_eq!("alpha", f.get_line(0, 100));
            assert_eq!("omega", f.get_line(1, 100));
        }
        std::fs::remove_file(path).unwrap();
    }
}
