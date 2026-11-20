//! DB value → JSON like Django's "python" serializer + `DjangoJSONEncoder`.
//!
//! upstream: django/core/serializers/python.py (`_value_from_field`: protected types are kept,
//! otherwise `value_to_string`), django/core/serializers/json.py (`DjangoJSONEncoder`: datetime →
//! isoformat, microseconds truncated to milliseconds, `+00:00` → `Z`; date → isoformat;
//! Decimal/UUID → str), psycopg 3 (inet as text, jsonb → json.loads).

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{Column, Row, TypeInfo, ValueRef};

/// `DjangoJSONEncoder` for aware datetimes (UTC connection).
pub fn django_datetime(t: &DateTime<Utc>) -> String {
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let micros = t.nanosecond() / 1000 % 1_000_000;
    if micros == 0 { format!("{base}Z") } else { format!("{base}.{:03}Z", micros / 1000) }
}

fn django_naive_datetime(t: &NaiveDateTime) -> String {
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let micros = t.nanosecond() / 1000 % 1_000_000;
    if micros == 0 { base } else { format!("{base}.{:03}", micros / 1000) }
}

fn django_time(t: &NaiveTime) -> String {
    let base = t.format("%H:%M:%S").to_string();
    let micros = t.nanosecond() / 1000 % 1_000_000;
    if micros == 0 { base } else { format!("{base}.{:03}", micros / 1000) }
}

/// PostgreSQL binary format of `inet`/`cidr` → text form (like `inet::text`, `/32`/`/128` is dropped).
fn inet_text(b: &[u8], cidr: bool) -> Option<String> {
    if b.len() < 4 {
        return None;
    }
    let (family, bits, nb) = (b[0], b[1], b[3] as usize);
    let addr = b.get(4..4 + nb)?;
    let (ip, max): (std::net::IpAddr, u8) = match family {
        2 if nb == 4 => (std::net::IpAddr::from([addr[0], addr[1], addr[2], addr[3]]), 32),
        3 if nb == 16 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(addr);
            (std::net::IpAddr::from(a), 128)
        }
        _ => return None,
    };
    Some(if bits == max && !cidr { ip.to_string() } else { format!("{ip}/{bits}") })
}

fn f64_value(f: f64) -> Value {
    serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
}

/// Column `i` of a row as a JSON value in the export format.
pub fn column_json(row: &PgRow, i: usize) -> Result<Value, sqlx::Error> {
    let raw = row.try_get_raw(i)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let ty = row.columns()[i].type_info().name().to_ascii_uppercase();
    Ok(match ty.as_str() {
        "INT2" => Value::from(row.try_get::<i16, _>(i)?),
        "INT4" => Value::from(row.try_get::<i32, _>(i)?),
        "INT8" => Value::from(row.try_get::<i64, _>(i)?),
        "FLOAT4" => f64_value(f64::from(row.try_get::<f32, _>(i)?)),
        "FLOAT8" => f64_value(row.try_get::<f64, _>(i)?),
        "BOOL" => Value::Bool(row.try_get::<bool, _>(i)?),
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "CHAR" => Value::String(row.try_get::<String, _>(i)?),
        "TIMESTAMPTZ" => Value::String(django_datetime(&row.try_get::<DateTime<Utc>, _>(i)?)),
        "TIMESTAMP" => Value::String(django_naive_datetime(&row.try_get::<NaiveDateTime, _>(i)?)),
        "DATE" => Value::String(row.try_get::<NaiveDate, _>(i)?.format("%Y-%m-%d").to_string()),
        "TIME" => Value::String(django_time(&row.try_get::<NaiveTime, _>(i)?)),
        "JSONB" | "JSON" => row.try_get::<Value, _>(i)?,
        "UUID" => Value::String(row.try_get::<uuid::Uuid, _>(i)?.hyphenated().to_string()),
        "INET" | "CIDR" => {
            let text_format = matches!(raw.format(), sqlx::postgres::PgValueFormat::Text);
            let bytes = raw.as_bytes().map_err(sqlx::Error::Decode)?;
            if text_format {
                // inet::text output (host without /32 or /128) = psycopg text
                Value::String(String::from_utf8_lossy(bytes).into_owned())
            } else {
                Value::String(inet_text(bytes, ty == "CIDR").ok_or_else(|| sqlx::Error::Decode("invalid inet value".into()))?)
            }
        }
        "NUMERIC" if matches!(raw.format(), sqlx::postgres::PgValueFormat::Text) => {
            // DjangoJSONEncoder: str(Decimal) = PostgreSQL text form
            Value::String(String::from_utf8_lossy(raw.as_bytes().map_err(sqlx::Error::Decode)?).into_owned())
        }
        other => {
            return Err(sqlx::Error::Decode(format!("unsupported column type {other} in export").into()));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn datetime_like_django_json_encoder() {
        let t = Utc.with_ymd_and_hms(2025, 1, 6, 8, 0, 0).unwrap();
        assert_eq!(django_datetime(&t), "2025-01-06T08:00:00Z");
        let t2 = t + chrono::Duration::microseconds(123_999);
        assert_eq!(django_datetime(&t2), "2025-01-06T08:00:00.123Z");
        let t3 = t + chrono::Duration::microseconds(999);
        assert_eq!(django_datetime(&t3), "2025-01-06T08:00:00.000Z");
    }

    #[test]
    fn inet_binary() {
        assert_eq!(inet_text(&[2, 32, 0, 4, 127, 0, 0, 1], false).as_deref(), Some("127.0.0.1"));
        assert_eq!(inet_text(&[2, 24, 0, 4, 10, 0, 0, 0], false).as_deref(), Some("10.0.0.0/24"));
        let mut v6 = vec![3, 128, 0, 16];
        v6.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(inet_text(&v6, false).as_deref(), Some("::1"));
    }
}
