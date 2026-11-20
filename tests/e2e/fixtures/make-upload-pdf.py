"""Creates fixtures/e2e-upload.pdf (1 page, text) with fpdf2 from the reference venv. Deterministic."""
import pathlib
from fpdf import FPDF

pdf = FPDF()
pdf.set_creation_date(__import__("datetime").datetime(2025, 1, 6, 8, 0, 0))
pdf.add_page()
pdf.set_font("Helvetica", size=16)
pdf.cell(0, 10, "PaperRust E2E Upload")
pdf.ln(12)
pdf.set_font("Helvetica", size=12)
pdf.multi_cell(0, 8, "Invoice E2E-0001 from ACME Corp. Total 42.00 CHF. Browser upload test document.")
pdf.output(str(pathlib.Path(__file__).with_name("e2e-upload.pdf")))
