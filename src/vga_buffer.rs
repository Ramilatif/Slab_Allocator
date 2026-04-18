/// VGA text mode driver for 80x25 color terminal output.
///
/// Provides a global [`WRITER`] protected by a spinlock, and the [`print!`] / [`println!`]
/// macros that delegate to it. Characters outside printable ASCII (0x20–0x7e) are
/// replaced with the `░` block character (0xfe).

use volatile::Volatile;
use core::fmt;
use spin::Mutex;

/// Standard 4-bit VGA color codes used for foreground and background.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0, Blue = 1, Green = 2, Cyan = 3,
    Red = 4, Magenta = 5, Brown = 6, LightGray = 7,
    DarkGray = 8, LightBlue = 9, LightGreen = 10, LightCyan = 11,
    LightRed = 12, Pink = 13, Yellow = 14, White = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    const fn new(fg: Color, bg: Color) -> Self {
        ColorCode((bg as u8) << 4 | (fg as u8))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii: u8,
    color: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// Writes characters to the VGA text buffer, handling scrolling and line wrapping.
///
/// Maintains the current column position and color. When the cursor reaches the end
/// of a line or a newline is written, the screen scrolls up by one row.
/// The VGA buffer at `0xb8000` is accessed directly on each write.
pub struct Writer {
    col: usize,
    color: ColorCode,
}

impl Writer {
    /// Returns a mutable reference to the VGA hardware buffer at `0xb8000`.
    ///
    /// # Safety
    /// Assumes the VGA buffer is identity-mapped at `0xb8000` by the bootloader.
    fn buffer(&self) -> &mut Buffer {
        unsafe { &mut *(0xb8000 as *mut Buffer) }
    }

    /// Writes a single byte to the VGA buffer.
    ///
    /// A newline (`\n`) triggers scrolling. Any other byte is placed at the current
    /// column on the last row, advancing the cursor. If the column reaches the screen
    /// width, a newline is inserted first.
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.col >= BUFFER_WIDTH {
                    self.new_line();
                }
                let row = BUFFER_HEIGHT - 1;
                let col = self.col;
                self.buffer().chars[row][col].write(ScreenChar {
                    ascii: byte,
                    color: self.color,
                });
                self.col += 1;
            }
        }
    }

    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let c = self.buffer().chars[row][col].read();
                self.buffer().chars[row - 1][col].write(c);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.col = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar { ascii: b' ', color: self.color };
        for col in 0..BUFFER_WIDTH {
            self.buffer().chars[row][col].write(blank);
        }
    }

    /// Writes a UTF-8 string slice to the VGA buffer.
    ///
    /// Only printable ASCII bytes (0x20–0x7e) and `\n` are rendered directly.
    /// All other bytes are replaced with `░` (0xfe) to signal unsupported characters.
    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                _ => self.write_byte(0xfe),
            }
        }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

/// Global VGA writer, protected by a spinlock for safe concurrent access.
pub static WRITER: Mutex<Writer> = Mutex::new(Writer {
    col: 0,
    color: ColorCode::new(Color::White, Color::Black),
});

/// Initializes the VGA driver.
///
/// The global [`WRITER`] is initialized statically, so this function currently
/// serves as an explicit initialization hook for future setup (e.g. cursor hiding).
pub fn init() {}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
/// Internal print function used by the [`print!`] and [`println!`] macros.
///
/// Locks the global [`WRITER`] and delegates to [`core::fmt::Write::write_fmt`].
///
/// # Panics
/// Panics if the formatter returns an error (should never happen with VGA output).
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    WRITER.lock().write_fmt(args).unwrap();
}
