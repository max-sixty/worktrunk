#[macro_export]
macro_rules! ok_or_return {
    ($expr:expr, $default_val:expr) => {
        match $expr {
            Ok(val) => val,
            Err(_) => {
                return $default_val;
            }
        }
    };
}

#[macro_export]
macro_rules! some_or_return {
    ($expr:expr, $default_val:expr) => {
        match $expr {
            Some(val) => val,
            None => {
                return $default_val;
            }
        }
    };
}
