use serde::{Deserialize, Deserializer, de::Error};

#[derive(Deserialize)]
#[serde(untagged)]
enum Decimal<T> {
    Text(String),
    Number(T),
}

pub fn rate<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    let value = match Decimal::<f64>::deserialize(deserializer)? {
        Decimal::Text(text) => text.parse::<f64>().map_err(D::Error::custom)?,
        Decimal::Number(number) => number,
    };
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(D::Error::custom("invalid public rate"))
    }
}

pub fn integer<'de, D: Deserializer<'de>, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de> + std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match Decimal::<T>::deserialize(deserializer)? {
        Decimal::Text(text) => text.parse().map_err(D::Error::custom),
        Decimal::Number(number) => Ok(number),
    }
}

pub fn optional_integer<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    Option::<Decimal<i64>>::deserialize(deserializer)?
        .map(|value| match value {
            Decimal::Text(text) => text.parse().map_err(D::Error::custom),
            Decimal::Number(number) => Ok(number),
        })
        .transpose()
}
