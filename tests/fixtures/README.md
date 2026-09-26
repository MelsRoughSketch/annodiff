# Saved review fixtures

`review.json` and `review-prompt.md` are fixed compatibility references for saved reviews and outgoing review text. They cover Japanese text, Markdown fences, Open/Done, Sent, history, and null slices.

`python3 tests/cli.py` verifies that the CLI can load and rewrite the state and produce the expected prompt. The fixtures were captured from the previous implementation; keep their contents fixed rather than regenerating them with the code under test.
