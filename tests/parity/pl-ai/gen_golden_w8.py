"""Golden data part 2 (W8): tokenizer/splitter, sentence splitting, index nodes, embedding and
streaming wire format, chat synthesis (compact/refine), vector index file, Django cache pickle.

All with the unchanged upstream code or the libraries of the reference venv
(llama_index 0.14.25, nltk, tiktoken 0.12, sqlite-vec 0.1.9) against the local mock
(tests/parity/pl-ai/mock_llm.py, 127.0.0.1) – no real LLM, no third-party service, no Django.

Invocation (repo root):
    PYTHONUTF8=1 reference/venv/Scripts/python.exe tests/parity/pl-ai/gen_golden_w8.py upstream/src crates/pl-ai/testdata
Output: testdata/golden_w8.json, testdata/llmindex_py.db
"""

import dataclasses
import json
import pickle
import shutil
import sys
import tempfile
import types
from pathlib import Path

SRC = Path(sys.argv[1]).resolve()
OUT = Path(sys.argv[2]).resolve()
sys.path.insert(0, str(SRC))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import mock_llm  # noqa: E402
from llama_index.core import Document as LlamaDocument  # noqa: E402
from llama_index.core.indices.prompt_helper import PromptHelper  # noqa: E402
from llama_index.core.node_parser import SimpleNodeParser  # noqa: E402
from llama_index.core.node_parser.text.utils import split_by_sentence_tokenizer  # noqa: E402
from llama_index.core.prompts import PromptTemplate  # noqa: E402
from llama_index.core.schema import MetadataMode  # noqa: E402
from llama_index.core.text_splitter import TokenTextSplitter  # noqa: E402
from llama_index.core.utils import get_tokenizer  # noqa: E402
from llama_index.core.vector_stores.utils import node_to_metadata_dict  # noqa: E402

from paperless_ai.prompts.context import ChatQaPromptContext  # noqa: E402
from paperless_ai.prompts.context import ChatRefinePromptContext  # noqa: E402
from paperless_ai.prompts.render import render_prompt  # noqa: E402

golden: dict = {}
tok = get_tokenizer()

LONG_DE = (
    "Sehr geehrte Damen und Herren. Wir danken Ihnen für Ihren Auftrag Nr. 12. Die Rechnung "
    "über CHF 1'234.50 ist bis zum 31. Mai 2024 zahlbar. Bei Fragen wenden Sie sich an J. Muster! "
    "Ist das klar? Ja... vielleicht. (Siehe Anhang.) \"Zitat.\" Weiter geht es mit Text, der "
    "keine Punkte hat und einfach weiterläuft, weiterläuft, weiterläuft; bis er endet.\n\n"
    "Zweiter Absatz: U.S.A. und z.B. Abkürzungen. 3.5 Prozent. e.V. Mitglied -- Ende.\n"
    "Neue Zeile ohne Punkt\n\n\nDritter Absatz nach drei Umbrüchen. ‘Kurz.’ «Lang.» Schluss"
)
LONG_EN = " ".join(
    f"Sentence number {i} talks about invoices, taxes; and other things." for i in range(60)
) + "\n" + "word " * 300 + "\nEnd."
SAMPLES = ["", "a", "hello world", "Hallo Welt äöü ß 😀 日本語", "<|endoftext|> x", LONG_DE, LONG_EN,
           "x" * 5000, "Zeile\n" * 400, "\t lead and trail   "]

golden["tokens"] = [[s, len(tok(s))] for s in SAMPLES]

golden["token_splitter"] = []
for s in SAMPLES[1:]:
    for cs, ov in [(16, 4), (50, 10), (200, 0), (1024, 200)]:
        sp = TokenTextSplitter(separator=" ", chunk_size=cs, chunk_overlap=ov)
        golden["token_splitter"].append({"text": s, "chunk_size": cs, "overlap": ov, "chunks": sp.split_text(s)})


def truncate_content(content, chunk_size, context_size):
    """1:1 from paperless_ai/indexing.py (without Django import)."""
    prompt_helper = PromptHelper(context_window=context_size, num_output=512, chunk_overlap_ratio=0.1,
                                 chunk_size_limit=chunk_size)
    splitter = TokenTextSplitter(separator=" ", chunk_size=chunk_size,
                                 chunk_overlap=min(200, chunk_size - 1))
    chunks = splitter.split_text(content)
    truncated = prompt_helper.truncate(prompt=PromptTemplate(template="{content}"), text_chunks=chunks,
                                       padding=5)
    return " ".join(truncated)


def truncate_embedding_query(content, chunk_size):
    splitter = TokenTextSplitter(separator=" ", chunk_size=chunk_size, chunk_overlap=0)
    c = splitter.split_text(content)
    return c[0] if c else ""


golden["truncate_content"] = []
for s in SAMPLES:
    for cs, ctx in [(1024, 8192), (64, 600), (100, 700), (1024, 1600)]:
        try:
            r = truncate_content(s, cs, ctx)
        except ValueError as e:
            r = {"error": str(e)}
        golden["truncate_content"].append({"text": s, "chunk_size": cs, "context_size": ctx, "result": r})
golden["truncate_embedding_query"] = [
    {"text": s, "chunk_size": cs, "result": truncate_embedding_query(s, cs)} for s in SAMPLES for cs in (8, 1024)
]

# ---- nltk Punkt (untrained) like llama_index split_by_sentence_tokenizer --------------------------
punkt = split_by_sentence_tokenizer()
PUNKT = [LONG_DE, LONG_EN, "Hello world. This is it.", "J. Smith went home. ok", "Am 12. März. Dann",
         "Ende!!! Neu? Ja.", "Nr. 5 und Nr. 6.", "Er sagte: \"Geh.\" Dann ging er.", "(Sent1.) Sent2.",
         "a.b.c. d", "Text...  Mehr", "  führend. Und", "Mr. Brown. Mrs. Green.", "3.5. Kapitel",
         "Satz.\nNeue Zeile. Und", "End. --next", "x. y", "A. B. C. D.", "Hallo. Welt."]
golden["punkt"] = [[s, punkt(s)] for s in PUNKT]

golden["sentence_splitter"] = []
for s in [LONG_DE, LONG_EN, "x" * 3000, "Kurz."]:
    for cs, ov in [(32, 8), (64, 20), (128, 0), (1024, 200)]:
        sp = SimpleNodeParser(chunk_size=cs, chunk_overlap=ov)
        golden["sentence_splitter"].append({"text": s, "chunk_size": cs, "overlap": ov, "chunks": sp.split_text(s)})

# ---- Index nodes like build_document_node --------------------------------------------------------


def build_nodes(doc_id, text, metadata, chunk_size):
    doc = LlamaDocument(id_=str(doc_id), text=text, metadata=metadata,
                        excluded_embed_metadata_keys=list(metadata.keys()),
                        excluded_llm_metadata_keys=["document_id"])
    parser = SimpleNodeParser(chunk_size=chunk_size, chunk_overlap=min(200, chunk_size - 1),
                              id_func=lambda i, d: f"{d.id_}-node-{i}")
    return parser.get_nodes_from_documents([doc])


META = {"document_id": "7", "title": "Rechnung März", "tags": ["Finance", "Ä-Tag"], "correspondent": "Swisscom",
        "document_type": None, "filename": "2024/rechnung.pdf", "storage_path": "Bob private",
        "archive_serial_number": 1007, "created": "2024-03-01", "added": "2025-01-06T08:00:00+00:00",
        "modified": "2025-01-06T08:00:00.123456+00:00"}
node_cases = []
for text, cs in [("Notes: \n\nContent:\n\nKurzer Text.", 1024), ("Notes: a,b\n\nContent:\n\n" + LONG_DE, 300),
                 ("Notes: \n\nContent:\n\n", 1024)]:
    nodes = build_nodes(7, text, dict(META), cs)
    node_cases.append({
        "text": text, "chunk_size": cs, "metadata": META,
        "nodes": [{"id": n.node_id, "text": n.text, "node_content": json.dumps(node_to_metadata_dict(
            n, remove_text=False, flat_metadata=False)), "llm_content": n.get_content(metadata_mode=MetadataMode.LLM),
            "embed_content": n.get_content(metadata_mode=MetadataMode.EMBED)} for n in nodes],
    })
golden["nodes"] = node_cases

# ---- Mock + wire format ------------------------------------------------------------------------
srv, port = mock_llm.start()
golden["mock_port_placeholder"] = "PORT"
base = f"http://127.0.0.1:{port}"


def take():
    out = list(mock_llm.CAPTURED)
    mock_llm.CAPTURED.clear()
    for e in out:
        e["path"] = e["path"]
    return out


from llama_index.embeddings.ollama import OllamaEmbedding  # noqa: E402
from llama_index.embeddings.openai_like import OpenAILikeEmbedding  # noqa: E402
from llama_index.llms.ollama import Ollama  # noqa: E402
from llama_index.llms.openai_like import OpenAILike  # noqa: E402

SYSTEM = "SYSTEM PROMPT"
wire: dict = {}
oe = OllamaEmbedding(model_name="embeddinggemma", base_url=base, ollama_additional_kwargs={"num_ctx": 8192})
r1 = oe.get_text_embedding_batch(["eins zwei", "drei"])
r2 = oe.get_query_embedding("frage eins")
wire["ollama_embed"] = {"requests": take(), "batch": r1, "query": r2}
texts = [f"text {i}" for i in range(12)]
oe.get_text_embedding_batch(texts)
wire["ollama_embed_batching"] = [len(e["body"]["input"]) for e in take()]

ae = OpenAILikeEmbedding(model_name="text-embedding-3-small", api_key="k", api_base=base + "/v1", timeout=120)
r1 = ae.get_text_embedding_batch(["eins zwei", "drei"])
r2 = ae.get_query_embedding("frage eins")
wire["openai_embed"] = {"requests": take(), "batch": r1, "query": r2}
ae.get_text_embedding_batch(texts * 10)
wire["openai_embed_batching"] = [len(e["body"]["input"]) for e in take()]

ol = Ollama(model="llama3.1", base_url=base, context_window=8192, request_timeout=120, system_prompt=SYSTEM,
            additional_kwargs={"seed": 1})
gen = ol.stream(PromptTemplate("Q: {q}"), q="hallo")
wire["ollama_stream"] = {"chunks": list(gen), "requests": take()}
oa = OpenAILike(model="gpt-3.5-turbo", api_base=base + "/v1", api_key="fake", timeout=120, is_chat_model=True,
                is_function_calling_model=True, system_prompt=SYSTEM, additional_kwargs={"seed": 1})
gen = oa.stream(PromptTemplate("Q: {q}"), q="hallo")
wire["openai_stream"] = {"chunks": list(gen), "requests": take()}
golden["wire"] = wire

# ---- Vector index + chat synthesis like paperless_ai/chat.py ------------------------------------------
from llama_index.core import VectorStoreIndex  # noqa: E402
from llama_index.core.query_engine import RetrieverQueryEngine  # noqa: E402
from llama_index.core.response_synthesizers import get_response_synthesizer  # noqa: E402
from llama_index.core.retrievers import VectorIndexRetriever  # noqa: E402
from llama_index.core.vector_stores.types import FilterOperator  # noqa: E402
from llama_index.core.vector_stores.types import MetadataFilter  # noqa: E402
from llama_index.core.vector_stores.types import MetadataFilters  # noqa: E402

from paperless_ai.vector_store import PaperlessSqliteVecVectorStore  # noqa: E402

tmp = Path(tempfile.mkdtemp(prefix="plai_w8_"))
DOCS = [
    (1, "Rechnung Swisscom", "Notes: \n\nContent:\n\nSwisscom Rechnung Mobile Abo März 2024 Betrag CHF 49."),
    (2, "Steuern 2024", "Notes: wichtig\n\nContent:\n\nSteuererklärung Kanton Zürich 2024, Abzüge, Einkommen."),
    (3, "Arztbrief", "Notes: \n\nContent:\n\n" + LONG_DE),
    (4, "Versicherung", "Notes: \n\nContent:\n\nKrankenkasse Prämie 2024 Rechnung Versicherung."),
]
all_nodes = []
store = PaperlessSqliteVecVectorStore(uri=str(tmp), embed_model_name="embeddinggemma")
for did, title, text in DOCS:
    meta = dict(META, document_id=str(did), title=title, modified=f"2025-01-0{did}T08:00:00+00:00")
    nodes = build_nodes(did, text, meta, 128)
    embs = oe.get_text_embedding_batch([n.get_content(metadata_mode=MetadataMode.EMBED) for n in nodes])
    for n, e in zip(nodes, embs, strict=True):
        n.embedding = e
    store.add(nodes)
    all_nodes.extend(nodes)
take()
golden["index_rows"] = [
    dict(r) for r in store._conn.execute(
        "SELECT id, document_id, node_content FROM documents ORDER BY id").fetchall()
]
golden["index_meta"] = dict(store._conn.execute("SELECT key, value FROM index_meta").fetchall())
golden["index_doc_meta"] = [list(r) for r in store._conn.execute(
    "SELECT document_id, modified FROM document_meta ORDER BY document_id").fetchall()]

index = VectorStoreIndex.from_vector_store(vector_store=store, embed_model=oe)
queries = []
for q, flt in [("Rechnung Swisscom", None), ("Steuern Zürich", ["2", "3"]), ("Prämie", ["4"]),
               ("nichts davon", [])]:
    filters = None if flt is None else MetadataFilters(filters=[
        MetadataFilter(key="document_id", operator=FilterOperator.IN, value=flt)])
    rt = VectorIndexRetriever(index=index, similarity_top_k=5, filters=filters)
    res = rt.retrieve(q)
    queries.append({"query": q, "filter": flt, "hits": [[n.node.node_id, n.score] for n in res]})
take()
golden["retrieval"] = queries

chats = []
for backend in ("ollama", "openai"):
    for q, lang, ctx, chunk in [("Was kostet das Abo?", None, 8192, 1024), ("Steuern?", "de-ch", 1500, 256)]:
        llm = Ollama(model="llama3.1", base_url=base, context_window=ctx, request_timeout=120,
                     system_prompt="SYS") if backend == "ollama" else OpenAILike(
            model="gpt-3.5-turbo", api_base=base + "/v1", api_key="fake", timeout=120, is_chat_model=True,
            is_function_calling_model=True, system_prompt="SYS")
        rt = VectorIndexRetriever(index=index, similarity_top_k=5, filters=MetadataFilters(filters=[
            MetadataFilter(key="document_id", operator=FilterOperator.NIN, value=[])]))
        top = rt.retrieve(q)
        synth = get_response_synthesizer(
            llm=llm, prompt_helper=PromptHelper(context_window=ctx, num_output=512, chunk_overlap_ratio=0.1,
                                                chunk_size_limit=chunk),
            text_qa_template=PromptTemplate(template=render_prompt(ChatQaPromptContext(output_language=lang))),
            refine_template=PromptTemplate(template=render_prompt(ChatRefinePromptContext(output_language=lang))),
            streaming=True)
        qe = RetrieverQueryEngine.from_args(retriever=rt, llm=llm, response_synthesizer=synth, streaming=True)
        resp = qe.query(q)
        out = list(resp.response_gen)
        chats.append({"backend": backend, "query": q, "lang": lang, "context_size": ctx, "chunk_size": chunk,
                      "top": [n.node.node_id for n in top], "stream": out, "requests": take()})
golden["chat_flow"] = chats
store.close()
shutil.copy(tmp / "llmindex.db", OUT / "llmindex_py.db")
shutil.rmtree(tmp, ignore_errors=True)

# ---- Django-Cache: pickle.dumps(SuggestionCacheData(...), 5) ---------------------------------------
mod = types.ModuleType("documents.caching")
pkg = types.ModuleType("documents")
sys.modules.setdefault("documents", pkg)
sys.modules["documents.caching"] = mod


@dataclasses.dataclass(frozen=True)
class SuggestionCacheData:
    classifier_version: int
    classifier_hash: str
    suggestions: dict


SuggestionCacheData.__module__ = "documents.caching"
mod.SuggestionCacheData = SuggestionCacheData
sugg = {"title": "Rechnung März", "tags": {"existing_ids": [3, 300], "new_names": ["Auto", "Ä"]},
        "correspondents": {"existing_ids": [], "new_names": []},
        "document_types": {"existing_ids": [70000], "new_names": ["x" * 300]},
        "storage_paths": {"existing_ids": [], "new_names": []}, "dates": ["2024-03-01"]}
pk = pickle.dumps(SuggestionCacheData(1002, "ollama:llama3.1:user=3", sugg), 5)
golden["pickle"] = {"suggestions": sugg, "backend": "ollama:llama3.1:user=3", "hex": pk.hex()}
empty = {"title": "", "tags": {"existing_ids": [], "new_names": []},
         "correspondents": {"existing_ids": [], "new_names": []},
         "document_types": {"existing_ids": [], "new_names": []},
         "storage_paths": {"existing_ids": [], "new_names": []}, "dates": []}
golden["pickle_empty"] = {"suggestions": empty, "backend": "b",
                          "hex": pickle.dumps(SuggestionCacheData(1002, "b", empty), 5).hex()}
golden["pickle_uuid_hex"] = pickle.dumps("0123456789abcdef0123456789abcdef", 5).hex()

srv.shutdown()
txt = json.dumps(golden, ensure_ascii=False, indent=1).replace(f"127.0.0.1:{port}", "127.0.0.1:PORT")
(OUT / "golden_w8.json").write_text(txt, encoding="utf-8")
print("ok", len(txt))
