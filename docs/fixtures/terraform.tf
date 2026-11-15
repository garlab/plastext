# Terraform and HCL highlighting
terraform {
  required_version = ">= 1.5"
}

variable "names" {
  type    = list(string)
  default = ["café", "demo"]
}

locals {
  enabled     = true
  retries     = 3
  upper_names = [for name in var.names : upper(name) if name != ""]
  message     = "hello ${var.names[0]}"
  // Heredocs contain literal text, expressions and template directives.
  banner = <<-EOT
    %{ for name in var.names ~}
    Hello ${upper(name)}!
    %{ endfor ~}
  EOT
}

/* A multiline comment
   with literal ${not_an_expression}. */
resource "terraform_data" "example" {
  input = {
    greeting = local.message
    attempts = local.retries
  }
  lifecycle {
    prevent_destroy = false
  }
}

output "summary" {
  value = local.enabled ? local.banner : null
}
