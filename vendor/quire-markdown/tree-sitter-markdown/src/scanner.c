#include "tree_sitter/parser.h"
#include <assert.h>
#include <ctype.h>
#include <string.h>
#include <wchar.h>
#include <wctype.h>

// For explanation of the tokens see grammar.js
typedef enum {
    LINE_ENDING,
    SOFT_LINE_ENDING,
    BLOCK_CLOSE,
    BLOCK_CONTINUATION,
    BLOCK_QUOTE_START,
    INDENTED_CHUNK_START,
    ATX_H1_MARKER,
    ATX_H2_MARKER,
    ATX_H3_MARKER,
    ATX_H4_MARKER,
    ATX_H5_MARKER,
    ATX_H6_MARKER,
    SETEXT_H1_UNDERLINE,
    SETEXT_H2_UNDERLINE,
    THEMATIC_BREAK,
    LIST_MARKER_MINUS,
    LIST_MARKER_PLUS,
    LIST_MARKER_STAR,
    LIST_MARKER_PARENTHESIS,
    LIST_MARKER_DOT,
    LIST_MARKER_MINUS_DONT_INTERRUPT,
    LIST_MARKER_PLUS_DONT_INTERRUPT,
    LIST_MARKER_STAR_DONT_INTERRUPT,
    LIST_MARKER_PARENTHESIS_DONT_INTERRUPT,
    LIST_MARKER_DOT_DONT_INTERRUPT,
    FENCED_CODE_BLOCK_START_BACKTICK,
    FENCED_CODE_BLOCK_START_TILDE,
    BLANK_LINE_START,
    FENCED_CODE_BLOCK_END_BACKTICK,
    FENCED_CODE_BLOCK_END_TILDE,
    HTML_BLOCK_1_START,
    HTML_BLOCK_1_END,
    HTML_BLOCK_2_START,
    HTML_BLOCK_3_START,
    HTML_BLOCK_4_START,
    HTML_BLOCK_5_START,
    HTML_BLOCK_6_START,
    HTML_BLOCK_7_START,
    CLOSE_BLOCK,
    NO_INDENTED_CHUNK,
    ERROR,
    TRIGGER_ERROR,
    TOKEN_EOF,
    MINUS_METADATA,
    PLUS_METADATA,
    PIPE_TABLE_START,
    PIPE_TABLE_LINE_ENDING,
    PARAGRAPH_PIPE_CHECK,
} TokenType;

// Description of a block on the block stack.
//
// LIST_ITEM is a list item with minimal indentation (content begins at indent
// level 2) while LIST_ITEM_MAX_INDENTATION represents a list item with maximal
// indentation without being considered a indented code block.
//
// ANONYMOUS represents any block that whose close is not handled by the
// external s.
typedef enum {
    BLOCK_QUOTE,
    INDENTED_CODE_BLOCK,
    LIST_ITEM,
    LIST_ITEM_1_INDENTATION,
    LIST_ITEM_2_INDENTATION,
    LIST_ITEM_3_INDENTATION,
    LIST_ITEM_4_INDENTATION,
    LIST_ITEM_5_INDENTATION,
    LIST_ITEM_6_INDENTATION,
    LIST_ITEM_7_INDENTATION,
    LIST_ITEM_8_INDENTATION,
    LIST_ITEM_9_INDENTATION,
    LIST_ITEM_10_INDENTATION,
    LIST_ITEM_11_INDENTATION,
    LIST_ITEM_12_INDENTATION,
    LIST_ITEM_13_INDENTATION,
    LIST_ITEM_14_INDENTATION,
    LIST_ITEM_MAX_INDENTATION,
    FENCED_CODE_BLOCK,
    ANONYMOUS,
} Block;

// Determines if a character is punctuation as defined by the markdown spec.
static bool is_punctuation(char chr) {
    return (chr >= '!' && chr <= '/') || (chr >= ':' && chr <= '@') ||
           (chr >= '[' && chr <= '`') || (chr >= '{' && chr <= '~');
}

// Returns the indentation level which lines of a list item should have at
// minimum. Should only be called with blocks for which `is_list_item` returns
// true.
static uint8_t list_item_indentation(Block block) {
    return (uint8_t)(block - LIST_ITEM + 2);
}

#define NUM_HTML_TAG_NAMES_RULE_1 3

static const char *const HTML_TAG_NAMES_RULE_1[NUM_HTML_TAG_NAMES_RULE_1] = {
    "pre", "script", "style"};

#define NUM_HTML_TAG_NAMES_RULE_7 62

static const char *const HTML_TAG_NAMES_RULE_7[NUM_HTML_TAG_NAMES_RULE_7] = {
    "address",  "article",    "aside",  "base",     "basefont", "blockquote",
    "body",     "caption",    "center", "col",      "colgroup", "dd",
    "details",  "dialog",     "dir",    "div",      "dl",       "dt",
    "fieldset", "figcaption", "figure", "footer",   "form",     "frame",
    "frameset", "h1",         "h2",     "h3",       "h4",       "h5",
    "h6",       "head",       "header", "hr",       "html",     "iframe",
    "legend",   "li",         "link",   "main",     "menu",     "menuitem",
    "nav",      "noframes",   "ol",     "optgroup", "option",   "p",
    "param",    "section",    "source", "summary",  "table",    "tbody",
    "td",       "tfoot",      "th",     "thead",    "title",    "tr",
    "track",    "ul"};

// For explanation of the tokens see grammar.js
#define QUIRE_SETEXT_HEADINGS false

static const bool paragraph_interrupt_symbols[] = {
    false, // LINE_ENDING,
    false, // SOFT_LINE_ENDING,
    false, // BLOCK_CLOSE,
    false, // BLOCK_CONTINUATION,
    true,  // BLOCK_QUOTE_START,
    false, // INDENTED_CHUNK_START,
    true,  // ATX_H1_MARKER,
    true,  // ATX_H2_MARKER,
    true,  // ATX_H3_MARKER,
    true,  // ATX_H4_MARKER,
    true,  // ATX_H5_MARKER,
    true,  // ATX_H6_MARKER,
    QUIRE_SETEXT_HEADINGS, // SETEXT_H1_UNDERLINE,
    QUIRE_SETEXT_HEADINGS, // SETEXT_H2_UNDERLINE,
    true,  // THEMATIC_BREAK,
    true,  // LIST_MARKER_MINUS,
    true,  // LIST_MARKER_PLUS,
    true,  // LIST_MARKER_STAR,
    true,  // LIST_MARKER_PARENTHESIS,
    true,  // LIST_MARKER_DOT,
    false, // LIST_MARKER_MINUS_DONT_INTERRUPT,
    false, // LIST_MARKER_PLUS_DONT_INTERRUPT,
    false, // LIST_MARKER_STAR_DONT_INTERRUPT,
    false, // LIST_MARKER_PARENTHESIS_DONT_INTERRUPT,
    false, // LIST_MARKER_DOT_DONT_INTERRUPT,
    true,  // FENCED_CODE_BLOCK_START_BACKTICK,
    true,  // FENCED_CODE_BLOCK_START_TILDE,
    true,  // BLANK_LINE_START,
    false, // FENCED_CODE_BLOCK_END_BACKTICK,
    false, // FENCED_CODE_BLOCK_END_TILDE,
    true,  // HTML_BLOCK_1_START,
    false, // HTML_BLOCK_1_END,
    true,  // HTML_BLOCK_2_START,
    true,  // HTML_BLOCK_3_START,
    true,  // HTML_BLOCK_4_START,
    true,  // HTML_BLOCK_5_START,
    true,  // HTML_BLOCK_6_START,
    false, // HTML_BLOCK_7_START,
    false, // CLOSE_BLOCK,
    false, // NO_INDENTED_CHUNK,
    false, // ERROR,
    false, // TRIGGER_ERROR,
    false, // EOF,
    false, // MINUS_METADATA,
    false, // PLUS_METADATA,
    false, // PIPE_TABLE_START, checked separately by table_interrupts
    false, // PIPE_TABLE_LINE_ENDING,
    false, // PARAGRAPH_PIPE_CHECK,
};

// State bitflags used with `Scanner.state`

// Currently matching (at the beginning of a line)
static const uint8_t STATE_MATCHING = 0x1 << 0;
// Last line break was inside a paragraph
static const uint8_t STATE_WAS_SOFT_LINE_BREAK = 0x1 << 1;
// Block should be closed after next line break
static const uint8_t STATE_CLOSE_BLOCK = 0x1 << 4;
// The first line of the open paragraph has been checked for a pipe
static const uint8_t STATE_PIPE_CHECKED = 0x1 << 5;
// The first line of the open paragraph has an unescaped pipe, so no table can
// interrupt it (Lezer attaches its table parser to such a paragraph)
static const uint8_t STATE_FIRST_LINE_PIPE = 0x1 << 6;
// A list marker ended at EOF, so its item still needs the empty line Lezer
// sees there
static const uint8_t STATE_EMPTY_ITEM_AT_EOF = 0x1 << 2;

static size_t roundup_32(size_t x) {
    x--;

    x |= x >> 1;
    x |= x >> 2;
    x |= x >> 4;
    x |= x >> 8;
    x |= x >> 16;

    x++;

    return x;
}

// Lezer's GFM table `endLeaf` check, which lets a table interrupt a paragraph:
// the current line has an unescaped pipe (`hasPipe`), the raw next line
// matches `delimiterLine`, and `parseRow` counts the same cells in both lines
// from the current line's base position.
typedef struct {
    size_t count;
    bool first;
    bool cell;
    bool escaped;
} RowCount;

typedef enum {
    DELIMITER_PREFIX,
    DELIMITER_AFTER_PIPE,
    DELIMITER_AFTER_PIPE_SPACE,
    DELIMITER_COLON,
    DELIMITER_DASHES,
    DELIMITER_AFTER_CELL,
    DELIMITER_INVALID,
} DelimiterState;

typedef struct {
    bool active;
    // 0: the current line, 1: the next line, 2: done
    uint8_t line;
    bool after_cr;
    bool pipe;
    size_t base;
    size_t index;
    size_t header;
    RowCount row;
    DelimiterState delimiter;
    size_t groups;
} TableRows;

static void row_count_reset(RowCount *row) {
    row->count = 0;
    row->first = true;
    row->cell = false;
    row->escaped = false;
}

static void row_count_feed(RowCount *row, int32_t c) {
    if (c == '|' && !row->escaped) {
        if (!row->first || row->cell) {
            row->count++;
        }
        row->first = false;
        row->cell = false;
    } else if (row->escaped || (c != ' ' && c != '\t')) {
        row->cell = true;
    }
    row->escaped = !row->escaped && c == '\\';
}

static size_t row_count_finish(const RowCount *row) {
    return row->count + (row->cell ? 1 : 0);
}

static bool js_space(int32_t c) {
    return c == ' ' || c == '\t' || c == '\v' || c == '\f' || c == '\r' ||
           c == 0xa0 || c == 0x1680 || (c >= 0x2000 && c <= 0x200a) ||
           c == 0x2028 || c == 0x2029 || c == 0x202f || c == 0x205f ||
           c == 0x3000 || c == 0xfeff;
}

static DelimiterState delimiter_feed(DelimiterState state, size_t *groups,
                                     int32_t c) {
    switch (state) {
        case DELIMITER_PREFIX:
            if (c == '>' || js_space(c)) return DELIMITER_PREFIX;
            if (c == '|') return DELIMITER_AFTER_PIPE_SPACE;
            break;
        case DELIMITER_AFTER_PIPE:
        case DELIMITER_AFTER_PIPE_SPACE:
            if (js_space(c)) return DELIMITER_AFTER_PIPE_SPACE;
            break;
        case DELIMITER_COLON:
            return c == '-' ? DELIMITER_DASHES : DELIMITER_INVALID;
        case DELIMITER_DASHES:
            if (c == '-') return DELIMITER_DASHES;
            if (c == ':') return DELIMITER_AFTER_CELL;
            if (js_space(c)) return DELIMITER_AFTER_CELL;
            if (c == '|') {
                (*groups)++;
                return DELIMITER_AFTER_PIPE;
            }
            return DELIMITER_INVALID;
        case DELIMITER_AFTER_CELL:
            if (js_space(c)) return DELIMITER_AFTER_CELL;
            if (c == '|') {
                (*groups)++;
                return DELIMITER_AFTER_PIPE;
            }
            return DELIMITER_INVALID;
        case DELIMITER_INVALID:
            return DELIMITER_INVALID;
    }
    if (c == ':') return DELIMITER_COLON;
    if (c == '-') return DELIMITER_DASHES;
    return DELIMITER_INVALID;
}

static bool delimiter_matches(DelimiterState state, size_t groups) {
    switch (state) {
        case DELIMITER_AFTER_PIPE:
        case DELIMITER_DASHES:
        case DELIMITER_AFTER_CELL:
            return groups > 0;
        default:
            return false;
    }
}

static void table_rows_start(TableRows *rows, size_t base) {
    rows->active = true;
    rows->line = 0;
    rows->after_cr = false;
    rows->pipe = false;
    rows->base = base;
    rows->index = 0;
    rows->header = 0;
    row_count_reset(&rows->row);
    rows->delimiter = DELIMITER_PREFIX;
    rows->groups = 0;
}

static void table_rows_end_line(TableRows *rows) {
    if (rows->line == 0) {
        rows->header = row_count_finish(&rows->row);
        row_count_reset(&rows->row);
    }
    rows->line++;
}

static void table_rows_feed(TableRows *rows, int32_t c) {
    if (rows->line > 1) {
        return;
    }
    if (c == '\n' && rows->after_cr) {
        rows->after_cr = false;
        return;
    }
    rows->after_cr = c == '\r';
    if (c == '\n' || c == '\r') {
        table_rows_end_line(rows);
        return;
    }
    if (rows->line == 0) {
        if (c == '|' && !rows->row.escaped) {
            rows->pipe = true;
        }
        row_count_feed(&rows->row, c);
        return;
    }
    rows->delimiter = delimiter_feed(rows->delimiter, &rows->groups, c);
    if (rows->index++ >= rows->base) {
        row_count_feed(&rows->row, c);
    }
}

static bool table_rows_interrupt(const TableRows *rows) {
    return rows->pipe && rows->line > 0 &&
           delimiter_matches(rows->delimiter, rows->groups) &&
           rows->header == row_count_finish(&rows->row);
}

typedef struct {
    // A stack of open blocks in the current parse state. `ordered` runs
    // parallel to `items` and marks list items opened by an ordered marker, so
    // the scanner knows the kind of every enclosing list like Lezer's `inList`.
    struct {
        size_t size;
        size_t capacity;
        Block *items;
        bool *ordered;
    } open_blocks;

    // Parser state flags
    uint8_t state;
    // Number of blocks that have been matched so far. Only changes during
    // matching and is reset after every line ending
    uint8_t matched;
    // Consumed but "unused" indentation. Sometimes a tab needs to be "split" to
    // be used in multiple tokens.
    uint8_t indentation;
    // The current column. Used to decide how many spaces a tab should equal
    uint8_t column;
    // The delimiter length of the currently open fenced code block
    uint8_t fenced_code_block_delimiter_length;

    bool simulate;

    // Transient, never serialized: characters advanced, and the Lezer table
    // interrupt check fed by every advance while it is active.
    size_t advanced;
    TableRows rows;
} Scanner;

static void reserve_blocks(Scanner *s, size_t count) {
    if (s->open_blocks.capacity >= count) {
        return;
    }
    size_t capacity = roundup_32(count < 8 ? 8 : count);
    void *items = realloc(s->open_blocks.items, sizeof(Block) * capacity);
    assert(items != NULL);
    s->open_blocks.items = items;
    void *ordered = realloc(s->open_blocks.ordered, sizeof(bool) * capacity);
    assert(ordered != NULL);
    s->open_blocks.ordered = ordered;
    s->open_blocks.capacity = capacity;
}

static void push_kind(Scanner *s, Block b, bool ordered) {
    reserve_blocks(s, s->open_blocks.size + 1);
    s->open_blocks.ordered[s->open_blocks.size] = ordered;
    s->open_blocks.items[s->open_blocks.size++] = b;
}

static void push_block(Scanner *s, Block b) { push_kind(s, b, false); }

static inline Block pop_block(Scanner *s) {
    return s->open_blocks.items[--s->open_blocks.size];
}

// A marker line ends at a line break or at the end of the file, as in Lezer.
static bool at_line_end(TSLexer *lexer) {
    return lexer->lookahead == '\n' || lexer->lookahead == '\r' ||
           lexer->eof(lexer);
}

static bool is_list_item(Block block) {
    return block >= LIST_ITEM && block <= LIST_ITEM_MAX_INDENTATION;
}

// Lezer's `inList`: whether any open block, matched on this line or not, is an
// item of a list of the given kind.
static bool in_list(const Scanner *s, bool ordered) {
    for (size_t i = 0; i < s->open_blocks.size; i++) {
        if (is_list_item(s->open_blocks.items[i]) &&
            s->open_blocks.ordered[i] == ordered) {
            return true;
        }
    }
    return false;
}

static void open_list_item(Scanner *s, TSLexer *lexer, Block b,
                           bool ordered) {
    if (s->simulate) {
        return;
    }
    push_kind(s, b, ordered);
    if (lexer->eof(lexer)) {
        s->state |= STATE_EMPTY_ITEM_AT_EOF;
    }
}

// Bytes of the packed list kind bits that follow the serialized blocks.
static size_t kind_bytes(size_t blocks_count) { return (blocks_count + 7) / 8; }

// Write the whole state of a Scanner to a byte buffer
static unsigned serialize(Scanner *s, char *buffer) {
    unsigned size = 0;
    buffer[size++] = (char)s->state;
    buffer[size++] = (char)s->matched;
    buffer[size++] = (char)s->indentation;
    buffer[size++] = (char)s->column;
    buffer[size++] = (char)s->fenced_code_block_delimiter_length;
    size_t blocks_count = s->open_blocks.size;
    // The buffer is fixed-size (TREE_SITTER_SERIALIZATION_BUFFER_SIZE), and deep
    // nesting (240+ open blocks) doesn't fit; writing past the end corrupts the
    // parser. Serialize nothing instead: restoring the empty state surfaces the
    // pathological input as ERROR nodes - visible and safe, unlike truncated
    // state (silently wrong trees) or the overflow (memory corruption). The
    // guard also keeps the block count within its one byte.
    if (size + 1 + blocks_count * sizeof(Block) + kind_bytes(blocks_count) >
        TREE_SITTER_SERIALIZATION_BUFFER_SIZE) {
        return 0;
    }
    buffer[size++] = (char)blocks_count;
    if (blocks_count > 0) {
        memcpy(&buffer[size], s->open_blocks.items,
               blocks_count * sizeof(Block));
        size += blocks_count * sizeof(Block);
        memset(&buffer[size], 0, kind_bytes(blocks_count));
        for (size_t i = 0; i < blocks_count; i++) {
            if (s->open_blocks.ordered[i]) {
                buffer[size + i / 8] |= (char)(1 << (i % 8));
            }
        }
        size += kind_bytes(blocks_count);
    }
    return size;
}

// Read the whole state of a Scanner from a byte buffer
// `serizalize` and `deserialize` should be fully symmetric.
static void deserialize(Scanner *s, const char *buffer, unsigned length) {
    s->open_blocks.size = 0;
    s->state = 0;
    s->matched = 0;
    s->indentation = 0;
    s->column = 0;
    s->fenced_code_block_delimiter_length = 0;
    if (length > 0) {
        size_t size = 0;
        s->state = (uint8_t)buffer[size++];
        s->matched = (uint8_t)buffer[size++];
        s->indentation = (uint8_t)buffer[size++];
        s->column = (uint8_t)buffer[size++];
        s->fenced_code_block_delimiter_length = (uint8_t)buffer[size++];
        size_t blocks_count = (uint8_t)buffer[size++];
        if (blocks_count > 0) {
            reserve_blocks(s, blocks_count);
            memcpy(s->open_blocks.items, &buffer[size],
                   blocks_count * sizeof(Block));
            size += blocks_count * sizeof(Block);
            for (size_t i = 0; i < blocks_count; i++) {
                s->open_blocks.ordered[i] =
                    (((uint8_t)buffer[size + i / 8]) >> (i % 8)) & 1;
            }
            s->open_blocks.size = blocks_count;
        }
    }
}

static void mark_end(Scanner *s, TSLexer *lexer) {
    if (!s->simulate) {
        lexer->mark_end(lexer);
    }
}

// Convenience function to emit the error token. This is done to stop invalid
// parse branches. Specifically:
// 1. When encountering a newline after a line break that ended a paragraph, and
// no new block
//    has been opened.
// 2. When encountering a new block after a soft line break.
// 3. When a `$._trigger_error` token is valid, which is used to stop parse
// branches through
//    normal tree-sitter grammar rules.
//
// See also the `$._soft_line_break` and `$._paragraph_end_newline` tokens in
// grammar.js
static bool error(TSLexer *lexer) {
    lexer->result_symbol = ERROR;
    return true;
}

// Advance the lexer one character
// Also keeps track of the current column, counting tabs as spaces with tab stop
// 4 See https://github.github.com/gfm/#tabs
static size_t advance(Scanner *s, TSLexer *lexer) {
    size_t size = 1;
    if (lexer->lookahead == '\t') {
        size = 4 - s->column;
        s->column = 0;
    } else {
        s->column = (s->column + 1) % 4;
    }
    s->advanced++;
    if (s->rows.active) {
        table_rows_feed(&s->rows, lexer->lookahead);
    }
    lexer->advance(lexer, false);
    return size;
}

// Finish the table interrupt check started at the line ending, reading the
// rest of the current line and the next line.
static bool table_interrupts(Scanner *s, TSLexer *lexer) {
    if (!s->rows.active) {
        return false;
    }
    while (s->rows.line < 2 && !lexer->eof(lexer)) {
        advance(s, lexer);
    }
    if (s->rows.line == 0) {
        table_rows_end_line(&s->rows);
    }
    return table_rows_interrupt(&s->rows);
}

// Emit the zero-width check that records whether the first line of a new
// paragraph has an unescaped pipe. `has_pipe` is negative when the rest of the
// line still has to be read.
static bool paragraph_pipe_check(Scanner *s, TSLexer *lexer, int has_pipe) {
    if (has_pipe < 0) {
        mark_end(s, lexer);
        has_pipe = 0;
        bool escaped = false;
        while (lexer->lookahead != '\n' && lexer->lookahead != '\r' &&
               !lexer->eof(lexer)) {
            if (lexer->lookahead == '|' && !escaped) {
                has_pipe = 1;
                break;
            }
            escaped = !escaped && lexer->lookahead == '\\';
            advance(s, lexer);
        }
    }
    s->state |= STATE_PIPE_CHECKED;
    if (has_pipe) {
        s->state |= STATE_FIRST_LINE_PIPE;
    }
    lexer->result_symbol = PARAGRAPH_PIPE_CHECK;
    return true;
}

static bool needs_pipe_check(Scanner *s, const bool *valid_symbols) {
    return !s->simulate && valid_symbols[PARAGRAPH_PIPE_CHECK] &&
           !(s->state & (STATE_PIPE_CHECKED | STATE_WAS_SOFT_LINE_BREAK));
}

// Try to match the given block, i.e. consume all tokens that belong to the
// block. These are
// 1. indentation for list items and indented code blocks
// 2. '>' for block quotes
// Returns true if the block is matched and false otherwise
static bool match(Scanner *s, TSLexer *lexer, Block block) {
    switch (block) {
        case INDENTED_CODE_BLOCK:
            while (s->indentation < 4) {
                if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    s->indentation += advance(s, lexer);
                } else {
                    break;
                }
            }
            if (s->indentation >= 4 && lexer->lookahead != '\n' &&
                lexer->lookahead != '\r') {
                s->indentation -= 4;
                return true;
            }
            break;
        case LIST_ITEM:
        case LIST_ITEM_1_INDENTATION:
        case LIST_ITEM_2_INDENTATION:
        case LIST_ITEM_3_INDENTATION:
        case LIST_ITEM_4_INDENTATION:
        case LIST_ITEM_5_INDENTATION:
        case LIST_ITEM_6_INDENTATION:
        case LIST_ITEM_7_INDENTATION:
        case LIST_ITEM_8_INDENTATION:
        case LIST_ITEM_9_INDENTATION:
        case LIST_ITEM_10_INDENTATION:
        case LIST_ITEM_11_INDENTATION:
        case LIST_ITEM_12_INDENTATION:
        case LIST_ITEM_13_INDENTATION:
        case LIST_ITEM_14_INDENTATION:
        case LIST_ITEM_MAX_INDENTATION:
            while (s->indentation < list_item_indentation(block)) {
                if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    s->indentation += advance(s, lexer);
                } else {
                    break;
                }
            }
            if (s->indentation >= list_item_indentation(block)) {
                s->indentation -= list_item_indentation(block);
                return true;
            }
            if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
                s->indentation = 0;
                return true;
            }
            break;
        case BLOCK_QUOTE:
            while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                s->indentation += advance(s, lexer);
            }
            if (lexer->lookahead == '>') {
                advance(s, lexer);
                s->indentation = 0;
                if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    s->indentation += advance(s, lexer) - 1;
                }
                return true;
            }
            break;
        case FENCED_CODE_BLOCK:
        case ANONYMOUS:
            return true;
    }
    return false;
}

static bool parse_fenced_code_block(Scanner *s, const char delimiter,
                                    TSLexer *lexer, const bool *valid_symbols) {
    // count the number of backticks
    uint8_t level = 0;
    while (lexer->lookahead == delimiter) {
        advance(s, lexer);
        level++;
    }
    mark_end(s, lexer);
    // If this is able to close a fenced code block then that is the only valid
    // interpretation. It can only close a fenced code block if the number of
    // backticks is at least the number of backticks of the opening delimiter.
    // Also it cannot be indented more than 3 spaces.
    if ((delimiter == '`' ? valid_symbols[FENCED_CODE_BLOCK_END_BACKTICK]
                          : valid_symbols[FENCED_CODE_BLOCK_END_TILDE]) &&
        s->indentation < 4 && level >= s->fenced_code_block_delimiter_length) {
        while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            advance(s, lexer);
        }
        if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
            s->fenced_code_block_delimiter_length = 0;
            lexer->result_symbol = delimiter == '`'
                                       ? FENCED_CODE_BLOCK_END_BACKTICK
                                       : FENCED_CODE_BLOCK_END_TILDE;
            return true;
        }
    }
    // If this could be the start of a fenced code block, check if the info
    // string contains any backticks.
    if ((delimiter == '`' ? valid_symbols[FENCED_CODE_BLOCK_START_BACKTICK]
                          : valid_symbols[FENCED_CODE_BLOCK_START_TILDE]) &&
        level >= 3) {
        bool info_string_has_backtick = false;
        if (delimiter == '`') {
            while (lexer->lookahead != '\n' && lexer->lookahead != '\r' &&
                   !lexer->eof(lexer)) {
                if (lexer->lookahead == '`') {
                    info_string_has_backtick = true;
                    break;
                }
                advance(s, lexer);
            }
        }
        // If it does not then choose to interpret this as the start of a fenced
        // code block.
        if (!info_string_has_backtick) {
            lexer->result_symbol = delimiter == '`'
                                       ? FENCED_CODE_BLOCK_START_BACKTICK
                                       : FENCED_CODE_BLOCK_START_TILDE;
            if (!s->simulate)
                push_block(s, FENCED_CODE_BLOCK);
            // Remember the length of the delimiter for later, since we need it
            // to decide whether a sequence of backticks can close the block.
            s->fenced_code_block_delimiter_length = level;
            s->indentation = 0;
            return true;
        }
    }
    return false;
}

static bool parse_star(Scanner *s, TSLexer *lexer, const bool *valid_symbols) {
    advance(s, lexer);
    mark_end(s, lexer);
    // Otherwise count the number of stars permitting whitespaces between them.
    size_t star_count = 1;
    // Also remember how many stars there are before the first whitespace...
    // ...and how many spaces follow the first star.
    uint8_t extra_indentation = 0;
    for (;;) {
        if (lexer->lookahead == '*') {
            if (star_count == 1 && extra_indentation >= 1 &&
                valid_symbols[LIST_MARKER_STAR]) {
                // If we get to this point then the token has to be at least
                // this long. We need to call `mark_end` here in case we decide
                // later that this is a list item.
                mark_end(s, lexer);
            }
            star_count++;
            advance(s, lexer);
        } else if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            if (star_count == 1) {
                extra_indentation += advance(s, lexer);
            } else {
                advance(s, lexer);
            }
        } else {
            break;
        }
    }
    bool line_end = at_line_end(lexer);
    bool dont_interrupt = false;
    if (star_count == 1 && line_end) {
        extra_indentation = 1;
        // line is empty so don't interrupt paragraphs if this is a list marker
        dont_interrupt = !in_list(s, false);
    }
    // If there were at least 3 stars then this could be a thematic break
    bool thematic_break = star_count >= 3 && line_end;
    // If there was a star and at least one space after that star then this
    // could be a list marker.
    bool list_marker_star = star_count >= 1 && extra_indentation >= 1;
    if (valid_symbols[THEMATIC_BREAK] && thematic_break && s->indentation < 4) {
        // If a thematic break is valid then it takes precedence
        lexer->result_symbol = THEMATIC_BREAK;
        mark_end(s, lexer);
        s->indentation = 0;
        return true;
    }
    if ((dont_interrupt ? valid_symbols[LIST_MARKER_STAR_DONT_INTERRUPT]
                        : valid_symbols[LIST_MARKER_STAR]) &&
        list_marker_star) {
        // List markers take precedence over emphasis markers
        // If star_count > 1 then we already called mark_end at the right point.
        // Otherwise the token should go until this point.
        if (star_count == 1) {
            mark_end(s, lexer);
        }
        // Not counting one space...
        extra_indentation--;
        // ... check if the list item begins with an indented code block
        if (extra_indentation <= 3) {
            // If not then calculate the indentation level of the list item
            // content as indentation of list marker + indentation after list
            // marker - 1
            extra_indentation += s->indentation;
            s->indentation = 0;
        } else {
            // Otherwise the indentation level is just the indentation of the
            // list marker. We keep the indentation after the list marker for
            // later blocks.
            uint8_t temp = s->indentation;
            s->indentation = extra_indentation;
            extra_indentation = temp;
        }
        open_list_item(s, lexer, (Block)(LIST_ITEM + extra_indentation), false);
        lexer->result_symbol =
            dont_interrupt ? LIST_MARKER_STAR_DONT_INTERRUPT : LIST_MARKER_STAR;
        return true;
    }
    return false;
}

static bool parse_thematic_break_underscore(Scanner *s, TSLexer *lexer,
                                            const bool *valid_symbols) {
    advance(s, lexer);
    mark_end(s, lexer);
    size_t underscore_count = 1;
    for (;;) {
        if (lexer->lookahead == '_') {
            underscore_count++;
            advance(s, lexer);
        } else if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            advance(s, lexer);
        } else {
            break;
        }
    }
    bool line_end = at_line_end(lexer);
    if (underscore_count >= 3 && line_end && valid_symbols[THEMATIC_BREAK]) {
        lexer->result_symbol = THEMATIC_BREAK;
        mark_end(s, lexer);
        s->indentation = 0;
        return true;
    }
    return false;
}

static bool parse_block_quote(Scanner *s, TSLexer *lexer,
                              const bool *valid_symbols) {
    if (valid_symbols[BLOCK_QUOTE_START]) {
        advance(s, lexer);
        s->indentation = 0;
        if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            s->indentation += advance(s, lexer) - 1;
        }
        lexer->result_symbol = BLOCK_QUOTE_START;
        if (!s->simulate)
            push_block(s, BLOCK_QUOTE);
        return true;
    }
    return false;
}

static bool parse_atx_heading(Scanner *s, TSLexer *lexer,
                              const bool *valid_symbols) {
    if (valid_symbols[ATX_H1_MARKER] && s->indentation <= 3) {
        mark_end(s, lexer);
        uint16_t level = 0;
        while (lexer->lookahead == '#' && level <= 6) {
            advance(s, lexer);
            level++;
        }
        if (level <= 6 &&
            (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
             lexer->lookahead == '\n' || lexer->lookahead == '\r' ||
             lexer->eof(lexer))) {
            lexer->result_symbol = ATX_H1_MARKER + (level - 1);
            s->indentation = 0;
            mark_end(s, lexer);
            return true;
        }
    }
    return false;
}

static bool parse_setext_underline(Scanner *s, TSLexer *lexer,
                                   const bool *valid_symbols) {
    if (QUIRE_SETEXT_HEADINGS && valid_symbols[SETEXT_H1_UNDERLINE] &&
        s->matched == s->open_blocks.size) {
        mark_end(s, lexer);
        while (lexer->lookahead == '=') {
            advance(s, lexer);
        }
        while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            advance(s, lexer);
        }
        if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
            lexer->result_symbol = SETEXT_H1_UNDERLINE;
            mark_end(s, lexer);
            return true;
        }
    }
    return false;
}

static bool parse_plus(Scanner *s, TSLexer *lexer, const bool *valid_symbols) {
    if (s->indentation <= 3 &&
        (valid_symbols[LIST_MARKER_PLUS] ||
         valid_symbols[LIST_MARKER_PLUS_DONT_INTERRUPT] ||
         valid_symbols[PLUS_METADATA])) {
        advance(s, lexer);
        if (valid_symbols[PLUS_METADATA] && lexer->lookahead == '+') {
            advance(s, lexer);
            if (lexer->lookahead != '+') {
                return false;
            }
            advance(s, lexer);
            while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                advance(s, lexer);
            }
            if (lexer->lookahead != '\n' && lexer->lookahead != '\r') {
                return false;
            }
            for (;;) {
                // advance over newline
                if (lexer->lookahead == '\r') {
                    advance(s, lexer);
                    if (lexer->lookahead == '\n') {
                        advance(s, lexer);
                    }
                } else {
                    advance(s, lexer);
                }
                // check for pluses
                size_t plus_count = 0;
                while (lexer->lookahead == '+') {
                    plus_count++;
                    advance(s, lexer);
                }
                if (plus_count == 3) {
                    // if exactly 3 check if next symbol (after eventual
                    // whitespace) is newline
                    while (lexer->lookahead == ' ' ||
                           lexer->lookahead == '\t') {
                        advance(s, lexer);
                    }
                    if (lexer->lookahead == '\r' || lexer->lookahead == '\n') {
                        // if so also consume newline
                        if (lexer->lookahead == '\r') {
                            advance(s, lexer);
                            if (lexer->lookahead == '\n') {
                                advance(s, lexer);
                            }
                        } else {
                            advance(s, lexer);
                        }
                        mark_end(s, lexer);
                        lexer->result_symbol = PLUS_METADATA;
                        return true;
                    }
                }
                // otherwise consume rest of line
                while (lexer->lookahead != '\n' && lexer->lookahead != '\r' &&
                       !lexer->eof(lexer)) {
                    advance(s, lexer);
                }
                // if end of file is reached, then this is not metadata
                if (lexer->eof(lexer)) {
                    break;
                }
            }
        } else {
            uint8_t extra_indentation = 0;
            while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                extra_indentation += advance(s, lexer);
            }
            bool dont_interrupt = false;
            if (at_line_end(lexer)) {
                extra_indentation = 1;
                dont_interrupt = !in_list(s, false);
            }
            if (extra_indentation >= 1 &&
                (dont_interrupt ? valid_symbols[LIST_MARKER_PLUS_DONT_INTERRUPT]
                                : valid_symbols[LIST_MARKER_PLUS])) {
                lexer->result_symbol = dont_interrupt
                                           ? LIST_MARKER_PLUS_DONT_INTERRUPT
                                           : LIST_MARKER_PLUS;
                extra_indentation--;
                if (extra_indentation <= 3) {
                    extra_indentation += s->indentation;
                    s->indentation = 0;
                } else {
                    uint8_t temp = s->indentation;
                    s->indentation = extra_indentation;
                    extra_indentation = temp;
                }
                open_list_item(s, lexer, (Block)(LIST_ITEM + extra_indentation), false);
                return true;
            }
        }
    }
    return false;
}

static bool parse_ordered_list_marker(Scanner *s, TSLexer *lexer,
                                      const bool *valid_symbols) {
    if (s->indentation <= 3 &&
        (valid_symbols[LIST_MARKER_PARENTHESIS] ||
         valid_symbols[LIST_MARKER_DOT] ||
         valid_symbols[LIST_MARKER_PARENTHESIS_DONT_INTERRUPT] ||
         valid_symbols[LIST_MARKER_DOT_DONT_INTERRUPT])) {
        size_t digits = 1;
        bool dont_interrupt = lexer->lookahead != '1';
        advance(s, lexer);
        while (isdigit(lexer->lookahead)) {
            dont_interrupt = true;
            digits++;
            advance(s, lexer);
        }
        if (digits >= 1 && digits <= 9) {
            bool dot = false;
            bool parenthesis = false;
            if (lexer->lookahead == '.') {
                advance(s, lexer);
                dot = true;
            } else if (lexer->lookahead == ')') {
                advance(s, lexer);
                parenthesis = true;
            }
            if (dot || parenthesis) {
                uint8_t extra_indentation = 0;
                while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    extra_indentation += advance(s, lexer);
                }
                if (at_line_end(lexer)) {
                    extra_indentation = 1;
                    dont_interrupt = true;
                }
                dont_interrupt = dont_interrupt && !in_list(s, true);
                if (extra_indentation >= 1 &&
                    (dot ? (dont_interrupt
                                ? valid_symbols[LIST_MARKER_DOT_DONT_INTERRUPT]
                                : valid_symbols[LIST_MARKER_DOT])
                         : (dont_interrupt
                                ? valid_symbols
                                      [LIST_MARKER_PARENTHESIS_DONT_INTERRUPT]
                                : valid_symbols[LIST_MARKER_PARENTHESIS]))) {
                    lexer->result_symbol =
                        dot ? LIST_MARKER_DOT : LIST_MARKER_PARENTHESIS;
                    extra_indentation--;
                    if (extra_indentation <= 3) {
                        extra_indentation += s->indentation;
                        s->indentation = 0;
                    } else {
                        uint8_t temp = s->indentation;
                        s->indentation = extra_indentation;
                        extra_indentation = temp;
                    }
                    open_list_item(
                            s, lexer,
                            (Block)(LIST_ITEM + extra_indentation + digits), true);
                    return true;
                }
            }
        }
    }
    return false;
}

static bool parse_minus(Scanner *s, TSLexer *lexer, const bool *valid_symbols) {
    if (s->indentation <= 3 &&
        (valid_symbols[LIST_MARKER_MINUS] ||
         valid_symbols[LIST_MARKER_MINUS_DONT_INTERRUPT] ||
         valid_symbols[SETEXT_H2_UNDERLINE] || valid_symbols[THEMATIC_BREAK] ||
         valid_symbols[MINUS_METADATA])) {
        mark_end(s, lexer);
        bool whitespace_after_minus = false;
        bool minus_after_whitespace = false;
        size_t minus_count = 0;
        uint8_t extra_indentation = 0;

        for (;;) {
            if (lexer->lookahead == '-') {
                if (minus_count == 1 && extra_indentation >= 1) {
                    mark_end(s, lexer);
                }
                minus_count++;
                advance(s, lexer);
                minus_after_whitespace = whitespace_after_minus;
            } else if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                if (minus_count == 1) {
                    extra_indentation += advance(s, lexer);
                } else {
                    advance(s, lexer);
                }
                whitespace_after_minus = true;
            } else {
                break;
            }
        }
        bool line_end = at_line_end(lexer);
        bool dont_interrupt = false;
        if (minus_count == 1 && line_end) {
            extra_indentation = 1;
            dont_interrupt = !in_list(s, false);
        }
        bool thematic_break = minus_count >= 3 && line_end;
        bool underline =
            minus_count >= 1 && !minus_after_whitespace && line_end &&
            s->matched ==
                s->open_blocks
                    .size; // setext heading can not break lazy continuation
        bool list_marker_minus = minus_count >= 1 && extra_indentation >= 1;
        bool success = false;
        if (QUIRE_SETEXT_HEADINGS && valid_symbols[SETEXT_H2_UNDERLINE] && underline) {
            lexer->result_symbol = SETEXT_H2_UNDERLINE;
            mark_end(s, lexer);
            s->indentation = 0;
            success = true;
        } else if (valid_symbols[THEMATIC_BREAK] &&
                   thematic_break) { // underline is false if list_marker_minus
                                     // is true
            lexer->result_symbol = THEMATIC_BREAK;
            mark_end(s, lexer);
            s->indentation = 0;
            success = true;
        } else if ((dont_interrupt
                        ? valid_symbols[LIST_MARKER_MINUS_DONT_INTERRUPT]
                        : valid_symbols[LIST_MARKER_MINUS]) &&
                   list_marker_minus) {
            if (minus_count == 1) {
                mark_end(s, lexer);
            }
            extra_indentation--;
            if (extra_indentation <= 3) {
                extra_indentation += s->indentation;
                s->indentation = 0;
            } else {
                uint8_t temp = s->indentation;
                s->indentation = extra_indentation;
                extra_indentation = temp;
            }
            open_list_item(s, lexer, (Block)(LIST_ITEM + extra_indentation), false);
            lexer->result_symbol = dont_interrupt
                                       ? LIST_MARKER_MINUS_DONT_INTERRUPT
                                       : LIST_MARKER_MINUS;
            return true;
        }
        if (minus_count == 3 && (!minus_after_whitespace) &&
            !lexer->eof(lexer) && line_end && valid_symbols[MINUS_METADATA]) {
            for (;;) {
                // advance over newline
                if (lexer->lookahead == '\r') {
                    advance(s, lexer);
                    if (lexer->lookahead == '\n') {
                        advance(s, lexer);
                    }
                } else {
                    advance(s, lexer);
                }
                // check for minuses
                minus_count = 0;
                while (lexer->lookahead == '-') {
                    minus_count++;
                    advance(s, lexer);
                }
                if (minus_count == 3) {
                    // if exactly 3 check if next symbol (after eventual
                    // whitespace) is newline
                    while (lexer->lookahead == ' ' ||
                           lexer->lookahead == '\t') {
                        advance(s, lexer);
                    }
                    if (lexer->lookahead == '\r' || lexer->lookahead == '\n') {
                        // if so also consume newline
                        if (lexer->lookahead == '\r') {
                            advance(s, lexer);
                            if (lexer->lookahead == '\n') {
                                advance(s, lexer);
                            }
                        } else {
                            advance(s, lexer);
                        }
                        mark_end(s, lexer);
                        lexer->result_symbol = MINUS_METADATA;
                        return true;
                    }
                }
                // otherwise consume rest of line
                while (lexer->lookahead != '\n' && lexer->lookahead != '\r' &&
                       !lexer->eof(lexer)) {
                    advance(s, lexer);
                }
                // if end of file is reached, then this is not metadata
                if (lexer->eof(lexer)) {
                    break;
                }
            }
        }
        if (success) {
            return true;
        }
    }
    return false;
}

static bool parse_html_block(Scanner *s, TSLexer *lexer,
                             const bool *valid_symbols) {
    if (!(valid_symbols[HTML_BLOCK_1_START] ||
          valid_symbols[HTML_BLOCK_1_END] ||
          valid_symbols[HTML_BLOCK_2_START] ||
          valid_symbols[HTML_BLOCK_3_START] ||
          valid_symbols[HTML_BLOCK_4_START] ||
          valid_symbols[HTML_BLOCK_5_START] ||
          valid_symbols[HTML_BLOCK_6_START] ||
          valid_symbols[HTML_BLOCK_7_START])) {
        return false;
    }
    advance(s, lexer);
    if (lexer->lookahead == '?' && valid_symbols[HTML_BLOCK_3_START]) {
        advance(s, lexer);
        lexer->result_symbol = HTML_BLOCK_3_START;
        if (!s->simulate)
            push_block(s, ANONYMOUS);
        return true;
    }
    if (lexer->lookahead == '!') {
        // could be block 2
        advance(s, lexer);
        if (lexer->lookahead == '-') {
            advance(s, lexer);
            if (lexer->lookahead == '-' && valid_symbols[HTML_BLOCK_2_START]) {
                advance(s, lexer);
                lexer->result_symbol = HTML_BLOCK_2_START;
                if (!s->simulate)
                    push_block(s, ANONYMOUS);
                return true;
            }
        } else if ('A' <= lexer->lookahead && lexer->lookahead <= 'Z' &&
                   valid_symbols[HTML_BLOCK_4_START]) {
            advance(s, lexer);
            lexer->result_symbol = HTML_BLOCK_4_START;
            if (!s->simulate)
                push_block(s, ANONYMOUS);
            return true;
        } else if (lexer->lookahead == '[') {
            advance(s, lexer);
            if (lexer->lookahead == 'C') {
                advance(s, lexer);
                if (lexer->lookahead == 'D') {
                    advance(s, lexer);
                    if (lexer->lookahead == 'A') {
                        advance(s, lexer);
                        if (lexer->lookahead == 'T') {
                            advance(s, lexer);
                            if (lexer->lookahead == 'A') {
                                advance(s, lexer);
                                if (lexer->lookahead == '[' &&
                                    valid_symbols[HTML_BLOCK_5_START]) {
                                    advance(s, lexer);
                                    lexer->result_symbol = HTML_BLOCK_5_START;
                                    if (!s->simulate)
                                        push_block(s, ANONYMOUS);
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    bool starting_slash = lexer->lookahead == '/';
    if (starting_slash) {
        advance(s, lexer);
    }
    char name[11];
    size_t name_length = 0;
    while (iswalpha((wint_t)lexer->lookahead)) {
        if (name_length < 10) {
            name[name_length++] = (char)towlower((wint_t)lexer->lookahead);
        } else {
            name_length = 12;
        }
        advance(s, lexer);
    }
    if (name_length == 0) {
        return false;
    }
    bool tag_closed = false;
    if (name_length < 11) {
        name[name_length] = 0;
        bool next_symbol_valid =
            lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
            lexer->lookahead == '\n' || lexer->lookahead == '\r' ||
            lexer->lookahead == '>';
        if (next_symbol_valid) {
            // try block 1 names
            for (size_t i = 0; i < NUM_HTML_TAG_NAMES_RULE_1; i++) {
                if (strcmp(name, HTML_TAG_NAMES_RULE_1[i]) == 0) {
                    if (starting_slash) {
                        if (valid_symbols[HTML_BLOCK_1_END]) {
                            lexer->result_symbol = HTML_BLOCK_1_END;
                            return true;
                        }
                    } else if (valid_symbols[HTML_BLOCK_1_START]) {
                        lexer->result_symbol = HTML_BLOCK_1_START;
                        if (!s->simulate)
                            push_block(s, ANONYMOUS);
                        return true;
                    }
                }
            }
        }
        if (!next_symbol_valid && lexer->lookahead == '/') {
            advance(s, lexer);
            if (lexer->lookahead == '>') {
                advance(s, lexer);
                tag_closed = true;
            }
        }
        if (next_symbol_valid || tag_closed) {
            // try block 2 names
            for (size_t i = 0; i < NUM_HTML_TAG_NAMES_RULE_7; i++) {
                if (strcmp(name, HTML_TAG_NAMES_RULE_7[i]) == 0 &&
                    valid_symbols[HTML_BLOCK_6_START]) {
                    lexer->result_symbol = HTML_BLOCK_6_START;
                    if (!s->simulate)
                        push_block(s, ANONYMOUS);
                    return true;
                }
            }
        }
    }

    if (!valid_symbols[HTML_BLOCK_7_START]) {
        return false;
    }

    if (!tag_closed) {
        // tag name (continued)
        while (iswalnum((wint_t)lexer->lookahead) || lexer->lookahead == '-') {
            advance(s, lexer);
        }
        if (!starting_slash) {
            // attributes
            bool had_whitespace = false;
            for (;;) {
                // whitespace
                while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    had_whitespace = true;
                    advance(s, lexer);
                }
                if (lexer->lookahead == '/') {
                    advance(s, lexer);
                    break;
                }
                if (lexer->lookahead == '>') {
                    break;
                }
                // attribute name
                if (!had_whitespace) {
                    return false;
                }
                if (!iswalpha((wint_t)lexer->lookahead) &&
                    lexer->lookahead != '_' && lexer->lookahead != ':') {
                    return false;
                }
                had_whitespace = false;
                advance(s, lexer);
                while (iswalnum((wint_t)lexer->lookahead) ||
                       lexer->lookahead == '_' || lexer->lookahead == '.' ||
                       lexer->lookahead == ':' || lexer->lookahead == '-') {
                    advance(s, lexer);
                }
                // attribute value specification
                // optional whitespace
                while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    had_whitespace = true;
                    advance(s, lexer);
                }
                // =
                if (lexer->lookahead == '=') {
                    advance(s, lexer);
                    had_whitespace = false;
                    // optional whitespace
                    while (lexer->lookahead == ' ' ||
                           lexer->lookahead == '\t') {
                        advance(s, lexer);
                    }
                    // attribute value
                    if (lexer->lookahead == '\'' || lexer->lookahead == '"') {
                        char delimiter = (char)lexer->lookahead;
                        advance(s, lexer);
                        while (lexer->lookahead != delimiter &&
                               lexer->lookahead != '\n' &&
                               lexer->lookahead != '\r' && !lexer->eof(lexer)) {
                            advance(s, lexer);
                        }
                        if (lexer->lookahead != delimiter) {
                            return false;
                        }
                        advance(s, lexer);
                    } else {
                        // unquoted attribute value
                        bool had_one = false;
                        while (lexer->lookahead != ' ' &&
                               lexer->lookahead != '\t' &&
                               lexer->lookahead != '"' &&
                               lexer->lookahead != '\'' &&
                               lexer->lookahead != '=' &&
                               lexer->lookahead != '<' &&
                               lexer->lookahead != '>' &&
                               lexer->lookahead != '`' &&
                               lexer->lookahead != '\n' &&
                               lexer->lookahead != '\r' && !lexer->eof(lexer)) {
                            advance(s, lexer);
                            had_one = true;
                        }
                        if (!had_one) {
                            return false;
                        }
                    }
                }
            }
        } else {
            while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                advance(s, lexer);
            }
        }
        if (lexer->lookahead != '>') {
            return false;
        }
        advance(s, lexer);
    }
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
        advance(s, lexer);
    }
    if (lexer->lookahead == '\r' || lexer->lookahead == '\n') {
        lexer->result_symbol = HTML_BLOCK_7_START;
        if (!s->simulate)
            push_block(s, ANONYMOUS);
        return true;
    }
    return false;
}

static bool parse_pipe_table(Scanner *s, TSLexer *lexer,
                             const bool *valid_symbols, int *has_pipe) {

    // unused
    (void)(valid_symbols);

    // PIPE_TABLE_START is zero width
    mark_end(s, lexer);
    // count number of cells
    size_t cell_count = 0;
    // also remember if we see starting and ending pipes, as empty headers have
    // to have both
    bool starting_pipe = false;
    bool ending_pipe = false;
    bool empty = true;
    if (lexer->lookahead == '|') {
        starting_pipe = true;
        advance(s, lexer);
    }
    while (lexer->lookahead != '\r' && lexer->lookahead != '\n' &&
           !lexer->eof(lexer)) {
        if (lexer->lookahead == '|') {
            cell_count++;
            ending_pipe = true;
            advance(s, lexer);
        } else {
            if (lexer->lookahead != ' ' && lexer->lookahead != '\t') {
                ending_pipe = false;
            }
            if (lexer->lookahead == '\\') {
                advance(s, lexer);
                if (is_punctuation((char)lexer->lookahead)) {
                    advance(s, lexer);
                }
            } else {
                advance(s, lexer);
            }
        }
    }
    *has_pipe = starting_pipe || cell_count > 0;
    if (empty && cell_count == 0 && !(starting_pipe && ending_pipe)) {
        return false;
    }
    if (!ending_pipe) {
        cell_count++;
    }

    // check the following line for a delimiter row
    // parse a newline
    if (lexer->lookahead == '\n') {
        advance(s, lexer);
    } else if (lexer->lookahead == '\r') {
        advance(s, lexer);
        if (lexer->lookahead == '\n') {
            advance(s, lexer);
        }
    } else {
        return false;
    }
    s->indentation = 0;
    s->column = 0;
    for (;;) {
        if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            s->indentation += advance(s, lexer);
        } else {
            break;
        }
    }
    s->simulate = true;
    uint8_t matched_temp = 0;
    while (matched_temp < (uint8_t)s->open_blocks.size) {
        if (match(s, lexer, s->open_blocks.items[matched_temp])) {
            matched_temp++;
        } else {
            return false;
        }
    }

    // check if delimiter row has the same number of cells and at least one pipe
    size_t delimiter_cell_count = 0;
    if (lexer->lookahead == '|') {
        advance(s, lexer);
    }
    for (;;) {
        while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            advance(s, lexer);
        }
        if (lexer->lookahead == '|') {
            delimiter_cell_count++;
            advance(s, lexer);
            continue;
        }
        if (lexer->lookahead == ':') {
            advance(s, lexer);
            if (lexer->lookahead != '-') {
                return false;
            }
        }
        bool had_one_minus = false;
        while (lexer->lookahead == '-') {
            had_one_minus = true;
            advance(s, lexer);
        }
        if (had_one_minus) {
            delimiter_cell_count++;
        }
        if (lexer->lookahead == ':') {
            if (!had_one_minus) {
                return false;
            }
            advance(s, lexer);
        }
        while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
            advance(s, lexer);
        }
        if (lexer->lookahead == '|') {
            if (!had_one_minus) {
                delimiter_cell_count++;
            }
            advance(s, lexer);
            continue;
        }
        if (lexer->lookahead != '\r' && lexer->lookahead != '\n') {
            return false;
        } else {
            break;
        }
    }
    // if the cell counts are not equal then this is not a table
    if (cell_count != delimiter_cell_count) {
        return false;
    }

    lexer->result_symbol = PIPE_TABLE_START;
    return true;
}

static bool scan(Scanner *s, TSLexer *lexer, const bool *valid_symbols) {
    // A normal tree-sitter rule decided that the current branch is invalid and
    // now "requests" an error to stop the branch
    if (valid_symbols[TRIGGER_ERROR]) {
        return error(lexer);
    }

    // Close the inner most block after the next line break as requested. See
    // `$._close_block` in grammar.js
    if (valid_symbols[CLOSE_BLOCK]) {
        s->state |= STATE_CLOSE_BLOCK;
        lexer->result_symbol = CLOSE_BLOCK;
        return true;
    }

    // if we are at the end of the file and there are still open blocks close
    // them all
    if (lexer->eof(lexer)) {
        if (valid_symbols[TOKEN_EOF]) {
            s->state &= ~(STATE_PIPE_CHECKED | STATE_FIRST_LINE_PIPE);
            lexer->result_symbol = TOKEN_EOF;
            return true;
        }
        if ((s->state & STATE_EMPTY_ITEM_AT_EOF) &&
            valid_symbols[BLANK_LINE_START]) {
            s->state &= ~STATE_EMPTY_ITEM_AT_EOF;
            lexer->result_symbol = BLANK_LINE_START;
            return true;
        }
        if (s->open_blocks.size > 0) {
            lexer->result_symbol = BLOCK_CLOSE;
            if (!s->simulate)
                pop_block(s);
            return true;
        }
        return false;
    }

    if (!(s->state & STATE_MATCHING)) {
        // Inside the first line of a paragraph no block can start, and the
        // previous token cannot be a backslash: the start of the line emits
        // the check unless a block-start parser or indentation consumed input.
        if (needs_pipe_check(s, valid_symbols) &&
            !valid_symbols[BLANK_LINE_START] &&
            !valid_symbols[PIPE_TABLE_START]) {
            return paragraph_pipe_check(s, lexer, -1);
        }
        // Parse any preceeding whitespace and remember its length. This makes a
        // lot of parsing quite a bit easier.
        for (;;) {
            if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                s->indentation += advance(s, lexer);
            } else {
                break;
            }
        }
        // We are not matching. This is where the parsing logic for most
        // "normal" token is. Most importantly parsing logic for the start of
        // new blocks.
        if (valid_symbols[INDENTED_CHUNK_START] &&
            !valid_symbols[NO_INDENTED_CHUNK]) {
            if (s->indentation >= 4 && lexer->lookahead != '\n' &&
                lexer->lookahead != '\r') {
                lexer->result_symbol = INDENTED_CHUNK_START;
                if (!s->simulate)
                    push_block(s, INDENTED_CODE_BLOCK);
                s->indentation -= 4;
                return true;
            }
        }
        // Decide which tokens to consider based on the first non-whitespace
        // character
        switch (lexer->lookahead) {
            case '\r':
            case '\n':
                if (valid_symbols[BLANK_LINE_START]) {
                    // A blank line token is actually just 0 width, so do not
                    // consume the characters
                    lexer->result_symbol = BLANK_LINE_START;
                    return true;
                }
                break;
            case '`':
                // A backtick could mark the beginning or ending of a fenced
                // code block.
                return parse_fenced_code_block(s, '`', lexer, valid_symbols);
            case '~':
                // A tilde could mark the beginning or ending of a fenced code
                // block.
                return parse_fenced_code_block(s, '~', lexer, valid_symbols);
            case '*':
                // A star could either mark  a list item or a thematic break.
                // This code is similar to the code for '_' and '+'.
                return parse_star(s, lexer, valid_symbols);
            case '_':
                return parse_thematic_break_underscore(s, lexer, valid_symbols);
            case '>':
                // A '>' could mark the beginning of a block quote
                return parse_block_quote(s, lexer, valid_symbols);
            case '#':
                // A '#' could mark a atx heading
                return parse_atx_heading(s, lexer, valid_symbols);
            case '=':
                // A '=' could mark a setext underline
                return parse_setext_underline(s, lexer, valid_symbols);
            case '+':
                // A '+' could be a list marker
                return parse_plus(s, lexer, valid_symbols);
            case '0':
            case '1':
            case '2':
            case '3':
            case '4':
            case '5':
            case '6':
            case '7':
            case '8':
            case '9':
                // A number could be a list marker (if followed by a dot or a
                // parenthesis)
                return parse_ordered_list_marker(s, lexer, valid_symbols);
            case '-':
                // A minus could mark a list marker, a thematic break or a
                // setext underline
                return parse_minus(s, lexer, valid_symbols);
            case '<':
                // A < could mark the beginning of a html block
                return parse_html_block(s, lexer, valid_symbols);
        }
        bool line_content = lexer->lookahead != '\r' &&
                            lexer->lookahead != '\n' && !lexer->eof(lexer);
        // Only a check that starts where this call started is zero-width;
        // otherwise the first token inside the paragraph emits it.
        bool check_here = needs_pipe_check(s, valid_symbols) &&
                          s->advanced == 0 && line_content;
        int has_pipe = -1;
        if (line_content && valid_symbols[PIPE_TABLE_START] &&
            !(s->state & STATE_WAS_SOFT_LINE_BREAK)) {
            if (parse_pipe_table(s, lexer, valid_symbols, &has_pipe)) {
                return true;
            }
        }
        if (check_here) {
            return paragraph_pipe_check(s, lexer, has_pipe);
        }
    } else { // we are in the state of trying to match all currently open blocks
        bool partial_success = false;
        while (s->matched < (uint8_t)s->open_blocks.size) {
            if (s->matched == (uint8_t)s->open_blocks.size - 1 &&
                (s->state & STATE_CLOSE_BLOCK)) {
                if (!partial_success)
                    s->state &= ~STATE_CLOSE_BLOCK;
                break;
            }
            if (match(s, lexer, s->open_blocks.items[s->matched])) {
                partial_success = true;
                s->matched++;
            } else {
                if (s->state & STATE_WAS_SOFT_LINE_BREAK) {
                    s->state &= (~STATE_MATCHING);
                }
                break;
            }
        }
        if (partial_success) {
            if (s->matched == s->open_blocks.size) {
                s->state &= (~STATE_MATCHING);
            }
            lexer->result_symbol = BLOCK_CONTINUATION;
            return true;
        }

        if (!(s->state & STATE_WAS_SOFT_LINE_BREAK)) {
            lexer->result_symbol = BLOCK_CLOSE;
            pop_block(s);
            if (s->matched == s->open_blocks.size) {
                s->state &= (~STATE_MATCHING);
            }
            return true;
        }
    }

    // The parser just encountered a line break. Setup the state correspondingly
    if ((valid_symbols[LINE_ENDING] || valid_symbols[SOFT_LINE_ENDING] ||
         valid_symbols[PIPE_TABLE_LINE_ENDING]) &&
        (lexer->lookahead == '\n' || lexer->lookahead == '\r')) {
        if (lexer->lookahead == '\r') {
            advance(s, lexer);
            if (lexer->lookahead == '\n') {
                advance(s, lexer);
            }
        } else {
            advance(s, lexer);
        }
        size_t line_start = s->advanced;
        s->indentation = 0;
        s->column = 0;
        if (!(s->state & STATE_CLOSE_BLOCK) &&
            (valid_symbols[SOFT_LINE_ENDING] ||
             valid_symbols[PIPE_TABLE_LINE_ENDING])) {
            lexer->mark_end(lexer);
            for (;;) {
                if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    s->indentation += advance(s, lexer);
                } else {
                    break;
                }
            }
            s->simulate = true;
            uint8_t matched_temp = s->matched;
            s->matched = 0;
            bool one_will_be_matched = false;
            while (s->matched < (uint8_t)s->open_blocks.size) {
                if (match(s, lexer, s->open_blocks.items[s->matched])) {
                    s->matched++;
                    one_will_be_matched = true;
                } else {
                    break;
                }
            }
            bool all_will_be_matched = s->matched == s->open_blocks.size;
            if (valid_symbols[SOFT_LINE_ENDING] &&
                !valid_symbols[PIPE_TABLE_LINE_ENDING] &&
                !(s->state & STATE_FIRST_LINE_PIPE) && s->indentation < 4) {
                size_t consumed = s->advanced - line_start;
                table_rows_start(&s->rows, consumed > s->indentation
                                               ? consumed - s->indentation
                                               : 0);
            }
            bool continues =
                !lexer->eof(lexer) &&
                !scan(s, lexer, paragraph_interrupt_symbols) &&
                !table_interrupts(s, lexer);
            s->rows.active = false;
            if (continues) {
                s->matched = matched_temp;
                // If the last line break ended a paragraph and no new block
                // opened, the last line break should have been a soft line
                // break Reset the counter for matched blocks
                s->matched = 0;
                s->indentation = 0;
                s->column = 0;
                // If there is at least one open block, we should be in the
                // matching state. Also set the matching flag if a
                // `$._soft_line_break_marker` can be emitted so it does get
                // emitted.
                if (one_will_be_matched) {
                    s->state |= STATE_MATCHING;
                } else {
                    s->state &= (~STATE_MATCHING);
                }
                if (valid_symbols[PIPE_TABLE_LINE_ENDING]) {
                    if (all_will_be_matched) {
                        lexer->result_symbol = PIPE_TABLE_LINE_ENDING;
                        return true;
                    }
                } else {
                    lexer->result_symbol = SOFT_LINE_ENDING;
                    // reset some state variables
                    s->state |= STATE_WAS_SOFT_LINE_BREAK;
                    return true;
                }
            } else {
                s->matched = matched_temp;
            }
            s->indentation = 0;
            s->column = 0;
        }
        if (valid_symbols[LINE_ENDING]) {
            // If the last line break ended a paragraph and no new block opened,
            // the last line break should have been a soft line break Reset the
            // counter for matched blocks
            s->matched = 0;
            // If there is at least one open block, we should be in the matching
            // state. Also set the matching flag if a
            // `$._soft_line_break_marker` can be emitted so it does get
            // emitted.
            if (s->open_blocks.size > 0) {
                s->state |= STATE_MATCHING;
            } else {
                s->state &= (~STATE_MATCHING);
            }
            // reset some state variables
            s->state &= ~(STATE_WAS_SOFT_LINE_BREAK | STATE_PIPE_CHECKED |
                          STATE_FIRST_LINE_PIPE);
            lexer->result_symbol = LINE_ENDING;
            return true;
        }
    }
    return false;
}

void *tree_sitter_quire_markdown_external_scanner_create(void) {
    Scanner *s = (Scanner *)malloc(sizeof(Scanner));
    s->open_blocks.items = NULL;
    s->open_blocks.ordered = NULL;
    s->open_blocks.capacity = 0;
#if defined(__STDC_VERSION__) && (__STDC_VERSION__ >= 201112L)
    _Static_assert(ATX_H6_MARKER == ATX_H1_MARKER + 5, "");
#else
    assert(ATX_H6_MARKER == ATX_H1_MARKER + 5);
#endif
    deserialize(s, NULL, 0);
    s->advanced = 0;
    s->rows.active = false;

    return s;
}

bool tree_sitter_quire_markdown_external_scanner_scan(void *payload, TSLexer *lexer,
                                                const bool *valid_symbols) {
    Scanner *scanner = (Scanner *)payload;
    scanner->simulate = false;
    scanner->advanced = 0;
    scanner->rows.active = false;
    return scan(scanner, lexer, valid_symbols);
}

unsigned tree_sitter_quire_markdown_external_scanner_serialize(void *payload,
                                                         char *buffer) {
    Scanner *scanner = (Scanner *)payload;
    return serialize(scanner, buffer);
}

void tree_sitter_quire_markdown_external_scanner_deserialize(void *payload,
                                                       const char *buffer,
                                                       unsigned length) {
    Scanner *scanner = (Scanner *)payload;
    deserialize(scanner, buffer, length);
}

void tree_sitter_quire_markdown_external_scanner_destroy(void *payload) {
    Scanner *scanner = (Scanner *)payload;
    free(scanner->open_blocks.items);
    free(scanner->open_blocks.ordered);
    free(scanner);
}
