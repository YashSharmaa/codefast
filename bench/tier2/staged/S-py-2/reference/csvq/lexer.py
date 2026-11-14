"""Turning query text into tokens."""

from dataclasses import dataclass

from .errors import ParseError

KEYWORDS = {"SELECT", "FROM", "WHERE", "GROUP", "BY", "ORDER", "ASC", "DESC", "LIMIT", "AS",
            "AND", "OR", "NOT", "IS", "NULL", "COUNT", "SUM", "MIN", "MAX", "AVG",
            "JOIN", "INNER", "LEFT", "ON", "TRUE", "FALSE",
            "CASE", "WHEN", "THEN", "ELSE", "END"}
OPERATORS = ["<=", ">=", "!=", "<>", "=", "<", ">", "(", ")", ",", "*", "+", "-", "/", "."]


@dataclass(frozen=True)
class Token:
    kind: str  # KW, NAME, INT, FLOAT, STR, OP, EOF
    value: object
    pos: int


def _quoted(text: str, i: int, quote: str) -> tuple[str, int]:
    """Read a quoted run starting at text[i] == quote, doubled quotes mean one."""
    out = []
    j = i + 1
    while True:
        if j >= len(text):
            raise ParseError("unterminated quoted text", text, i)
        ch = text[j]
        if ch == quote:
            if j + 1 < len(text) and text[j + 1] == quote:
                out.append(quote)
                j += 2
                continue
            return "".join(out), j + 1
        out.append(ch)
        j += 1


def tokenize(text: str) -> list[Token]:
    tokens = []
    i = 0
    while i < len(text):
        ch = text[i]
        if ch.isspace():
            i += 1
        elif ch == "'":
            value, i2 = _quoted(text, i, "'")
            tokens.append(Token("STR", value, i))
            i = i2
        elif ch == '"':
            value, i2 = _quoted(text, i, '"')
            tokens.append(Token("NAME", value, i))
            i = i2
        elif ch.isascii() and ch.isdigit():
            j = i
            while j < len(text) and text[j].isascii() and text[j].isdigit():
                j += 1
            if j + 1 < len(text) and text[j] == "." and text[j + 1].isascii() and text[j + 1].isdigit():
                j += 1
                while j < len(text) and text[j].isascii() and text[j].isdigit():
                    j += 1
                tokens.append(Token("FLOAT", float(text[i:j]), i))
            else:
                tokens.append(Token("INT", int(text[i:j]), i))
            i = j
        elif ch == "_" or (ch.isascii() and ch.isalpha()):
            j = i
            while j < len(text) and (text[j] == "_" or (text[j].isascii() and text[j].isalnum())):
                j += 1
            word = text[i:j]
            if word.upper() in KEYWORDS:
                tokens.append(Token("KW", word.upper(), i))
            else:
                tokens.append(Token("NAME", word, i))
            i = j
        else:
            for op in OPERATORS:
                if text.startswith(op, i):
                    tokens.append(Token("OP", "!=" if op == "<>" else op, i))
                    i += len(op)
                    break
            else:
                raise ParseError(f"unexpected character {ch!r}", text, i)
    tokens.append(Token("EOF", None, len(text)))
    return tokens
