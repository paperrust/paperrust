# Custom fields

[Deutsch](../custom-fields.md) · [Documentation overview](README.md)

<a id="custom-fields"></a>

## Custom fields

Title, date, tags and correspondent are not always enough. Custom fields let you give documents
your own details, such as an invoice number, an amount or a due date.

### How it works

1. **Create the field:** in the management area under **Custom fields**, create a new field. It needs
   a **Name** (e.g. "Invoice number") and a **Data type** (see below).
2. **Use it on a document:** in the document view click **Custom fields** and pick the field. It
   appears in the form and you enter the value. PaperRust checks that the value fits the type.
3. **Take it off again:** point at the field name and choose **Remove**.
4. **Save:** new or removed fields and changed values only count after **Save** – like every other
   change in the document view.

A document can have many fields, but each field only once.

Other ways:

- **Many documents at once:** select documents in the document list and set or remove fields there.
- **Automatically:** a [workflow](workflows.md#workflow-action-assignment) sets or removes fields.
- **Search and filter:** in the document list you can filter by field values (for example "amount
  greater than 100"). The same condition is available as a filter in workflows.
- **In file names and titles:** field values can be used in [storage paths](storage-paths.md#additional-variables)
  and [workflow placeholders](workflows.md#workflow-placeholders).

> [!NOTE]
> You choose the data type when you create the field. After that it cannot be changed. The name
> can be changed at any time.

### Data types

| Type in the interface | Technical name | What it takes |
|---|---|---|
| **Text** | `string` | short text, one line |
| **Long text** | `longtext` | text over several lines |
| **Yes/no** | `boolean` | box ticked or not |
| **Date** | `date` | a date |
| **URL** | `url` | a valid web address |
| **Whole number** | `integer` | number without decimals, e.g. `12` |
| **Number** | `float` | number with decimals, e.g. `12.3456` |
| **Amount** | `monetary` | an amount of money (see below) |
| **Document link** | `documentlink` | links to other documents (see below) |
| **Select** | `select` | one entry from a fixed list (see below) |

**Amount:** a number with at most two decimals. It may start with a three-letter
[ISO 4217 currency code](https://en.wikipedia.org/wiki/ISO_4217#List_of_ISO_4217_currency_codes),
e.g. `EUR49.90`, or `CHF-12.5` for a negative amount. With a code the amount needs a point and
decimals (`EUR49.00`, not `EUR49`). Without a code the field's **Default currency**
applies; if none is set, the one from your language settings.

**Document link:** connects the document with one or more others. They are shown as links. The
connection works both ways: if A points to B, B automatically gets a link back to A.

**Select:** when creating the field you add the allowed entries with **Add choice**. On the document
you then pick one of them. If you later delete an entry from the list, documents that had exactly
this entry lose their value in this field.
