<?php
namespace yii\base;
class BaseObject { public function __construct(array $config = []) {} public function init(): void {} }
class Component extends BaseObject {
    public function on(string $name, callable $handler): void {}
    public function trigger(string $name): void {}
}
class Model extends Component {
    public function validate(): bool { return true; }
    public function beforeValidate(): bool { return true; }
    public function afterValidate(): void {}
    public function setAttributes(array $values, bool $safeOnly = true): void {}
}
namespace yii\db;
interface QueryInterface { public function all(): array; }
class Query extends \yii\base\BaseObject implements QueryInterface { public function all(): array { return []; } }
class ActiveRecord extends \yii\base\Model {
    public function save(bool $runValidation = true): bool { return true; }
    public function beforeSave(bool $insert): bool { return true; }
    public function afterSave(bool $insert, array $changedAttributes): void {}
    public function delete(): void {}
    public function beforeDelete(): bool { return true; }
    public function afterDelete(): void {}
    public static function findOne(mixed $condition): ?static { return null; }
    public function afterFind(): void {}
    public function refresh(): bool { return true; }
    public function afterRefresh(): void {}
}
class Command {
    /** @throws \RuntimeException */
    public function queryAll(): array { return []; }
    /** @throws \RuntimeException */
    public function queryOne(): array { return []; }
}
namespace yii;
class BaseYii { public static function createObject(string|array $type): object { return new \stdClass(); } }
