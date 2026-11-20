# File names and storage paths

[Deutsch](../storage-paths.md) · [Overview](README.md)

This page explains how PaperRust names the files in the media folder and how you can choose the
names yourself: for all documents at once, or with storage paths for particular groups of documents.

<a id="file-name-handling"></a><a id="dateinamen"></a>

## How PaperRust names files

Out of the box, each file is named after the document's number, padded to seven digits with
zeros: document 42 is stored as `0000042.pdf`. That is perfectly fine for working with PaperRust,
because you open documents through the web interface, not through the folder.

There are two ways to get readable names and folders:

- **For all documents:** the setting
  [`PAPERRUST_FILENAME_FORMAT`](configuration.md#PAPERRUST_FILENAME_FORMAT).
- **For certain documents:** a [storage path](#storage-paths). It wins over the setting.

Both are small templates made of plain text and [placeholders](#placeholders) in double curly
braces. A `/` creates a folder. PaperRust adds the file extension (`.pdf`, `.png` …) itself, so
leave it out of the template.

An example:

```bash
PAPERRUST_FILENAME_FORMAT={{ created_year }}/{{ document_type }}/{{ title }}
```

The media folder then looks roughly like this:

```
2024/
  Invoice/
    Electricity bill.pdf
    Garage oil change.pdf
  Contract/
    Lease.pdf
2025/
  Invoice/
    Electricity bill.pdf
    Electricity bill_01.pdf
```

The searchable archive version of a document gets the same name with `.pdf` and is kept in the
archive folder. If a document has several file versions, each version gets the same name plus
`_v1`, `_v2` and so on.

### What PaperRust takes care of

- **Forbidden characters** such as `/ \ : * ? " < > |` in titles or names become `-`.
- **Same name twice:** if two documents would end up with the same name, the second gets `_01`,
  the next `_02`, and so on. With versions the counter comes last: `contract_v2_01.pdf`.
- **Broken template:** if the template cannot be evaluated, PaperRust uses the default name
  (`0000042.pdf`) and writes the error to the log.
- **Path too long:** if the path gets too long (over 1024 characters, or longer than your file
  system allows), the file stays where it is and the log says why. Be careful with
  `{{ tag_list }}` on documents with many tags.
- **Inside the media folder only:** paths that would lead out of the media folder (`..`, an
  absolute path or a `\`) are refused, and PaperRust uses the default name instead.
- **Old notation:** templates with single braces such as `{created_year}/{title}` still work;
  PaperRust converts them itself.

### When files are renamed

PaperRust checks a document's file name every time the document is saved and moves the file if
needed. If you change or delete a storage path, all documents that use it are re-filed
automatically.

If you change `PAPERRUST_FILENAME_FORMAT`, however, existing documents stay put until you run the
[renamer `document_renamer`](administration.md#renamer) once.

> [!WARNING]
> Never move or rename files in the media folder by hand. PaperRust remembers the file name of
> every document. If the file is somewhere else, PaperRust can no longer find it and reports it
> as missing.

<a id="filename-format-variables"></a><a id="placeholders"></a>

## Placeholders

You can use these placeholders in any template. The examples assume a document created on
7 March 2025 and added on 12 March 2025.

| Placeholder | Content | Example |
|---|---|---|
| `{{ title }}` | title | `Electricity bill` |
| `{{ correspondent }}` | correspondent name | `City utilities` |
| `{{ document_type }}` | document type name | `Invoice` |
| `{{ tag_list }}` | all tags, sorted by name, separated by commas | `Electricity,House` |
| `{{ asn }}` | archive serial number (ASN) | `1207` |
| `{{ owner_username }}` | owner's user name | `anna` |
| `{{ original_name }}` | name of the uploaded file, without extension | `scan_0815` |
| `{{ doc_pk }}` | document number, seven digits | `0000042` |
| `{{ created }}` | creation date | `2025-03-07` |
| `{{ created_year }}` | year, four digits | `2025` |
| `{{ created_year_short }}` | year, two digits | `25` |
| `{{ created_month }}` | month as a number | `03` |
| `{{ created_month_name }}` | month name (English) | `March` |
| `{{ created_month_name_short }}` | short month name (English) | `Mar` |
| `{{ created_day }}` | day | `07` |
| `{{ added }}` | date added | `2025-03-12` |
| `{{ added_year }}` | year added | `2025` |
| `{{ added_year_short }}` | year, two digits | `25` |
| `{{ added_month }}` | month as a number | `03` |
| `{{ added_month_name }}` | month name (English) | `March` |
| `{{ added_month_name_short }}` | short month name (English) | `Mar` |
| `{{ added_day }}` | day | `12` |

Notes:

- If a value is missing (no correspondent, no ASN, no owner …), you get `none` in its place. The
  next section shows how to avoid that.
- Month names are always English. For month names in another language use the
  [`localize_date`](#date-localization) filter.
- The date added follows the time zone from `PAPERRUST_TIME_ZONE`.
- For more options see [templates with conditions and filters](#filename-templates) and the
  [additional variables](#additional-variables) there.

<a id="empty-placeholders"></a>

### Leaving out empty placeholders

Take the template `{{ correspondent }}/{{ title }}` and a document without a correspondent.
Normally it ends up as `none/Electricity bill.pdf`.

Turn on
[`PAPERRUST_FILENAME_FORMAT_REMOVE_NONE`](configuration.md#PAPERRUST_FILENAME_FORMAT_REMOVE_NONE)
and the `none` disappears: the document is stored directly as `Electricity bill.pdf` in the media
folder. No empty folder is created, and a space right before the empty placeholder goes away too.

<a id="storage-paths"></a>

## Storage paths

A storage path is a named template that only applies to the documents it is assigned to. This
lets you file different kinds of documents in different ways.

You find storage paths in the sidebar under **Attributes → Storage paths**. A storage path has:

- a **Name** you pick it by,
- a **Path**, which is the template (same rules and placeholders as above),
- [automatic matching](advanced.md#matching) settings, just like tags or correspondents.

Under **Preview** you choose an existing document and immediately see the file name it would get.
PaperRust checks the template when you save. If it contains an unknown placeholder or an error,
it cannot be saved.

A storage path is assigned automatically when a document is added and the matching rules fit. You
can change it on the document by hand at any time. Documents without a storage path follow
`PAPERRUST_FILENAME_FORMAT` or get the default name.

An example with two storage paths:

```
Taxes      = Taxes/{{ created_year }}/{{ correspondent }} {{ created }} {{ title }}
Household  = Household/{{ document_type }}/{{ title }}
```

Result:

```text
Taxes/
  2024/
    Tax office 2024-06-18 Assessment.pdf
    Health insurer 2024-02-01 Contribution statement.pdf
Household/
  Invoice/
    Fridge.pdf
  Warranty/
    Fridge.pdf
```

<a id="filename-templates"></a>

## Templates with conditions and filters

Templates can do more than fill in placeholders. They use the syntax of Jinja, a widely used
template language:

- `{{ … }}` outputs a value.
- `{% if … %} … {% elif … %} … {% else %} … {% endif %}` picks a part depending on a condition.
- A `|` passes a value through a filter, e.g. `{{ title | upper }}` for capital letters. The usual
  Jinja filters such as `upper`, `lower`, `replace`, `truncate` or `default` are available, plus
  [PaperRust's own filters](#custom-jinja2-filters).

A template may span several lines. PaperRust turns the result into one line: line breaks are
dropped, and so are spaces right before and after a `/`. So you can indent templates to keep them
readable.

Templates run in a protected sandbox. They can only read what PaperRust provides and cannot change
the document or any files. PaperRust stops templates that take too much work; the file then gets
the default name.

Besides the placeholders there is a `document` object with these fields:

- `document.id` or `document.pk`, `document.title`, `document.content` (the recognised text),
  `document.page_count`
- `document.created` (a date), `document.added`, `document.modified` (date and time)
- `document.archive_serial_number`, `document.mime_type` (e.g. `application/pdf`)
- `document.checksum`, `document.archive_checksum`
- `document.filename`, `document.archive_filename`, `document.original_filename`
- `document.correspondent`, `document.document_type`, `document.storage_path`: each with `.name`
  and `.id`, or empty when nothing is assigned
- `document.owner.username`, `document.tags` (list with `.name` and `.id`)

<a id="additional-variables"></a>

### Additional variables

- `{{ tag_name_list }}`: the names of all tags as a **list**, not as text. Handy for checks like
  `{% if "Taxes" in tag_name_list %}`.
- `{{ custom_fields }}`: all [custom fields](custom-fields.md) of the document, by field name.
  Each field has `.type` and `.value`. Use `{% if "Project" in custom_fields %}` to check whether a
  field is set and `{{ custom_fields.Project.value }}` to read it. For names with spaces, the
  [`get_cf_value`](#custom-field-access) filter is the better choice.

<a id="custom-jinja2-filters"></a>

### PaperRust's own filters

<a id="custom-field-access"></a>

#### `get_cf_value`: value of a custom field

```jinja
{{ custom_fields | get_cf_value('Invoice number') }}
{{ custom_fields | get_cf_value('Invoice number', 'no-number') }}
```

The left side is always `custom_fields`; inside the brackets go the field name and, if you like,
a fallback value. The fallback is used when the field is missing on the document or empty. With
neither a value nor a fallback, the name contains `None`. The filter also works with spaces in
the field name.

<a id="datetime-formatting"></a>

#### `datetime`: format a date freely

```jinja
{{ created | datetime('%d.%m.%Y') }}          → 07.03.2025
{{ created | datetime('%Y/%m') }}             → 2025/03
```

The filter takes a date or a date text such as `2025-03-07` and formats it with `%` codes. The
most useful ones: `%Y` year, `%y` two-digit year, `%m` month, `%d` day, `%H` hour, `%M` minute,
`%j` day of the year. Times from `document.added` and `document.modified` are in UTC. Names (`%B`
month, `%A` weekday) come out in English. Text that is not a date causes an error and therefore
the default name.

<a id="date-localization"></a>

#### `localize_date`: a date in a given language

```jinja
{{ document.created | localize_date('long', 'de_DE') }}       → 7. März 2025
{{ document.created | localize_date('MMMM yyyy', 'fr_FR') }}  → mars 2025
{{ document.created | localize_date('medium', 'en_US') }}     → Mar 7, 2025
```

The first value in the brackets is the format, the second the language with country (`en_US`,
`en_GB`, `de_DE`, `fr_FR` …). The format is either one of the [presets](#format-presets) or your
own pattern. In your own patterns letters stand for parts of the date: `d` day, `dd` two-digit
day, `MM` month as a number, `MMM` short month name, `MMMM` full month name, `yy` and `yyyy`
year, `EEEE` weekday.

The filter needs a real date, so use `document.created` or `document.added` rather than the text
placeholders. A date text in ISO format (`2025-03-07`) also works. An unknown language causes an
error and therefore the default name.

<a id="format-presets"></a>

#### Presets for `localize_date`

| Format | English (`en_US`) | German (`de_DE`) |
|---|---|---|
| `short` | `3/7/25` | `07.03.25` |
| `medium` | `Mar 7, 2025` | `07.03.2025` |
| `long` | `March 7, 2025` | `7. März 2025` |
| `full` | `Friday, March 7, 2025` | `Freitag, 7. März 2025` |

#### `slugify`: simplify text for file names

```jinja
{{ correspondent | slugify }}   → "Müller & Söhne GmbH" becomes muller-sohne-gmbh
```

Lower-cases everything, turns accented letters into plain ones, removes special characters and
puts hyphens in place of spaces. Useful when the files also end up on systems with strict rules
for file names.

<a id="examples"></a>

### Examples

**Folders by document type, with a catch-all folder.** A missing document type counts as "not set":

```jinja
{% if document_type %}{{ document_type }}{% else %}Unsorted{% endif %}/{{ created_year }}/{{ title }}
```

An invoice from 2025 lands in `Invoice/2025/Electricity bill.pdf`, a document without a type in
`Unsorted/2025/…`.

**Picking out tax papers by tag:**

```jinja
{% if "Taxes" in tag_name_list %}
  Taxes/{{ created_year }}
{% else %}
  Files/{{ correspondent }}
{% endif %}
/{{ title }}
```

With the tag "Taxes" this gives e.g. `Taxes/2024/Assessment.pdf`, otherwise
`Files/City utilities/Electricity bill.pdf`.

**By archive serial number, padded to five digits:**

```jinja
{% if document.archive_serial_number %}
  ASN/{{ "%05d" | format(document.archive_serial_number) }} {{ title }}
{% else %}
  no-ASN/{{ title }}
{% endif %}
```

ASN 1207 gives `ASN/01207 Electricity bill.pdf`.

**By file type:**

```jinja
{% if document.mime_type == "application/pdf" %}PDF{% else %}Images{% endif %}/{{ title }}
```

**Invoices by invoice date from a custom field:**

```jinja
Invoices/
{{ custom_fields | get_cf_value('Invoice date', created) | datetime('%Y/%m') }}/
{{ correspondent }} {{ custom_fields | get_cf_value('Invoice number', 'no-number') }}
```

If "Invoice date" is 28 February 2025 and "Invoice number" is `R-4711`, this becomes
`Invoices/2025/02/City utilities R-4711.pdf`. Without an invoice date the template falls back to
the creation date.

**Month folders with names in another language:**

```jinja
{{ created_year }}/{{ created_month }} {{ document.created | localize_date('MMMM', 'de_DE') }}/{{ title }}
```

Gives `2025/03 März/Electricity bill.pdf`. The number in front keeps the folders in the right
order.

> [!NOTE]
> Coming from paperless-ngx: you can take over existing templates and storage paths, PaperRust
> evaluates them the same way. A few rarely used Jinja pieces are missing (the filters `random`,
> `urlize`, `wordwrap`, `xmlattr`); a template using them falls back to the default name.
