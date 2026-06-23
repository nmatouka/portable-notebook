# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "marimo",
#     "numpy",
# ]
# ///

import marimo

__generated_with = "0.23.10"
app = marimo.App(width="medium")


@app.cell
def _(mo):
    mo.md("# Offline compound-interest demo")
    return


@app.cell
def _():
    import marimo as mo
    import numpy as np
    return mo, np


@app.cell
def _(mo):
    rate = mo.ui.slider(1, 20, value=5, step=1, label="Interest rate (%)")
    rate
    return (rate,)


@app.cell
def _(mo, np, rate):
    principal = 1000.0
    years = np.arange(0, 11)
    values = principal * (1 + rate.value / 100) ** years
    final = values[-1]
    mo.md(
        f"At **{rate.value}%**, \\$1,000 grows to **\\${final:,.2f}** over 10 years "
        f"(numpy computed {len(years)} yearly values)."
    )
    return


if __name__ == "__main__":
    app.run()
