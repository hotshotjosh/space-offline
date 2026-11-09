"""Checks for tools/metrics_sample.py: an invented exposition body, no network."""

from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import metrics_sample as ms  # noqa: E402

BODY = (
    "# HELP eve_uptime_seconds Seconds since start.\n"
    "# TYPE eve_uptime_seconds gauge\n"
    "eve_uptime_seconds 42\n"
    'eve_system_balls{system="39000001"} 7\n'
    'eve_method_calls{method="a\\"b} c\\\\d\\n"} 3\n'
    "\n"
)


class Sample(unittest.TestCase):
    def test_parses_plain_labelled_and_escaped_lines(self) -> None:
        got = ms.parse_exposition(BODY)
        self.assertEqual(
            got,
            [
                ("eve_uptime_seconds", "", "42"),
                ("eve_system_balls", 'system="39000001"', "7"),
                ("eve_method_calls", 'method="a\\"b} c\\\\d\\n"', "3"),
            ],
        )

    def test_rows_are_four_tab_separated_columns(self) -> None:
        rows = ms.rows_for(1000, BODY)
        self.assertEqual(rows[0], "1000\teve_uptime_seconds\t\t42\n")
        self.assertEqual(rows[1], '1000\teve_system_balls\tsystem="39000001"\t7\n')
        for r in rows:
            self.assertEqual(len(r.rstrip("\n").split("\t")), 4, r)

    def test_a_raw_tab_in_a_label_does_not_add_a_column(self) -> None:
        rows = ms.rows_for(1, 'm{method="a\tb"} 1\n')
        self.assertEqual(len(rows[0].rstrip("\n").split("\t")), 4)


if __name__ == "__main__":
    unittest.main()
