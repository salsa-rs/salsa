use syn::visit_mut::VisitMut;

pub(crate) struct ChangeLt<'a> {
    from: Option<&'a str>,
    to: String,
}

impl<'a> ChangeLt<'a> {
    pub fn elided_to(db_lt: &syn::Lifetime) -> Self {
        ChangeLt {
            from: Some("_"),
            to: db_lt.ident.to_string(),
        }
    }
    pub fn in_type(mut self, ty: &syn::Type) -> syn::Type {
        let mut ty = ty.clone();
        self.visit_type_mut(&mut ty);
        ty
    }
}

impl syn::visit_mut::VisitMut for ChangeLt<'_> {
    fn visit_lifetime_mut(&mut self, i: &mut syn::Lifetime) {
        if self.from.map(|f| i.ident == f).unwrap_or(true) {
            i.ident = syn::Ident::new(&self.to, i.ident.span());
        }
    }
}
