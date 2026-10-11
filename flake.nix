{
  description = "Logos bitcoin_wallet_cli: headless Bitcoin wallet sessions for logosctl, relayed to bitcoin_wallet_backend.";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    # bitcoin_wallet_backend is reached through its contract under contracts/, named by
    # dependency_overrides in metadata.json.
  };

  outputs = inputs@{ self, logos-module-builder, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      # x86_64-windows is a cross build from x86_64-linux.
      targets = systems ++ [ "x86_64-windows" ];
      forAllSystems = f: nixpkgs.lib.genAttrs targets f;
    in
    {
      packages = forAllSystems (system:
        (logos-module-builder.lib.mkLogosModule {
          src = ./.;
          configFile = ./metadata.json;
          flakeInputs = inputs;
        }).packages.${system});
    };
}
