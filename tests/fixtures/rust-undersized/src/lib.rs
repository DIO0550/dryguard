pub struct Invoice {
    amount: u32,
    tax: u32,
}

impl Invoice {
    pub fn amount(&self) -> u32 {
        self.amount
    }

    pub fn tax(&self) -> u32 {
        self.tax
    }

    pub fn total_with_discount(&self, rate: u32) -> u32 {
        let discounted = self.amount * (100 - rate) / 100;
        discounted + self.tax
    }

    pub fn total_with_surcharge(&self, fee: u32) -> u32 {
        let charged = self.amount * (100 + fee) / 100;
        charged + self.tax
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_with_no_discount_adds_the_tax() {
        let invoice = Invoice { amount: 100, tax: 10 };
        assert_eq!(invoice.total_with_discount(0), 110);
    }
}
