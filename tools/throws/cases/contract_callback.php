<?php
declare(strict_types=1);
namespace ThrowsProbe\contract_callback;
function caller(): void { \VendorProbe\invoke(static function (): void { throw new \DomainException(); }); }
function deferred(): void { \VendorProbe\defer(static function (): void { throw new \DomainException(); }); }
