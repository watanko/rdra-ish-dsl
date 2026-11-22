# Python domain representations

This model keeps nested records, ordered collections, nullable defaults, and exact
wire values in the DSL. `python.json` selects Python representations without repeating
fields or allowed values.

```sh
rdra-ish check samples/python-domain
rdra-ish codegen samples/python-domain --lang py \
  --python-config samples/python-domain/python.json -o /tmp/domain.py
```

`Checklist` and `ChecklistItem` are mutable dataclasses with slots. Each checklist
gets its own item list. `Settings` is an immutable Pydantic model that rejects extra
fields and unknown model names. Pydantic 2 is a dependency of code that imports that
opt-in output; the compiler and its default Python output do not require it.

`literal_value_sets` preserves Python `Literal` contracts for attributes. Lifecycle
tag enums retain their members so generated transitions and row conversion keep the
same state representation.
