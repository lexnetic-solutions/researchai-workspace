# Fixture Study: Retrieval Quality

This fixture document exercises the ingestion pipeline: headings become
sections, paragraphs become chunks with character offsets.

## Background

Retrieval quality depends on semantic chunking and hybrid search. Chunks
must preserve their source location for citation traceability.

## Method

We import this file, parse it through the document engine, and store
sections and chunks in SQLite.

## Expected Results

The assembled text must contain this paragraph, and the duplicate
checksum test must skip a second import of the same file.
