/// Records one or more fields in the current span.
///
/// Use % for recording a field with a [`Display`] value.
///
/// Use ? for recording a field with a [`Debug`] value.
///
/// # Examples
///
/// ```rs
/// fields!(user_name = user.name, system_id = %system.id, member_id = ?member.id);
/// ```
#[macro_export]
macro_rules! fields {
    // recursive cases
    ($name:tt = %$value:expr, $($rest:tt)?) => {
            ::tracing::span::Span::current()
                .record(::std::stringify!($name), ::tracing::field::display($value));

            fields!($($rest)+);
    };

    ($name:tt = ?$value:expr, $($rest:tt)?) => {
            ::tracing::span::Span::current()
                .record(::std::stringify!($name), ::tracing::field::debug($value));

            fields!($($rest)+);
    };

    ($name:tt = $value:expr, $($rest:tt)?) => {
            ::tracing::span::Span::current()
                .record(::std::stringify!($name), $value);

            fields!($($rest)+);
    };

    // base cases
    ($name:tt = %$value:expr) => {
            ::tracing::span::Span::current()
                .record(::std::stringify!($name), ::tracing::field::display($value));
    };

    ($name:tt = ?$value:expr) => {
            ::tracing::span::Span::current()
                .record(::std::stringify!($name), ::tracing::field::debug($value));
    };

    ($name:tt = $value:expr) => {
            ::tracing::span::Span::current()
                .record(::std::stringify!($name), $value);
    };
}
