// An enum-level index takes no `filter`; a partial index is declared on the
// model that embeds the enum.

#[derive(Debug, toasty::Embed)]
#[unique(name, filter = "name IS NOT NULL")]
enum Creature {
    #[column(variant = 1)]
    Human {
        #[shared(name)]
        name: String,
    },
    #[column(variant = 2)]
    Animal {
        #[shared(name)]
        name: String,
    },
}

fn main() {}
