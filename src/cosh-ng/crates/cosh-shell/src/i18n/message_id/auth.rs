macro_rules! auth_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            AuthSelectProviderQuestion,
        );
    };
}
